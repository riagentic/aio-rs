//! Work off the page's thread (`aio::effect::off_thread`): Web Workers
//! running the app's own module (`worker.js`, which `ax build --web`
//! writes), handed the compiled module, a function and its input. Up to
//! a core less than the device has run at once; more wait their turn. One
//! cancelled, or whose effect is dropped, is stopped at once (its worker
//! ends); one that panics ends its worker too. The worker's one export
//! ([`work`]) runs the steps the page names: a page that sends no work
//! links none of them.
use js_sys::{Array, Function, Promise, Reflect, Uint8Array};
use std::cell::RefCell;
use std::collections::VecDeque;
use wasm_bindgen::prelude::*;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// A function's runner: `f`'s table index and its input as JSON; its output as JSON.
pub(crate) type Runner = fn(usize, &[u8]) -> Result<Vec<u8>, String>;

/// A worker's step on its argument ([`work`]), sent by its table index.
type Step = fn(JsValue) -> Result<JsValue, JsValue>;

struct Job {
    id: u64,
    key: String,
    message: Array,
    input: Uint8Array,
    done: Function,
    failed: Function,
}

#[derive(Default)]
struct Pool {
    idle: Vec<web_sys::Worker>,
    running: Vec<(Job, web_sys::Worker)>,
    waiting: VecDeque<Job>,
    next: u64,
}

thread_local! {
    static POOL: RefCell<Pool> = RefCell::default();
}

/// How many run at once: a core less than the device has, at least one.
fn most() -> usize {
    let cores = web_sys::window().map_or(1.0, |w| w.navigator().hardware_concurrency());
    (cores as usize).saturating_sub(1).max(1)
}

/// Run `f` (by `run`) on `input` in a worker, under `key`.
pub(crate) async fn run(key: &str, run: Runner, f: usize, input: Vec<u8>) -> Result<Vec<u8>, String> {
    let (mut done, mut failed) = (None, None);
    let p = Promise::new(&mut |d, f| (done, failed) = (Some(d), Some(f)));
    let (Some(done), Some(failed)) = (done, failed) else { return Err("no promise".into()) };
    let input = Uint8Array::from(input.as_slice());
    let step = |s: Step, arg: &JsValue| Array::of2(&(s as usize).into(), arg);
    let loaded: Array = super::lazy::loaded().into_iter().map(JsValue::from).collect();
    let last = step(job, &Array::of3(&(run as usize).into(), &f.into(), &input));
    let steps = match loaded.length() {
        0 => Array::of1(&last),
        _ => Array::of2(&step(chunks, &loaded), &last),
    };
    let message = Array::of2(&wasm_bindgen::module(), &steps);
    let id = POOL.with_borrow_mut(|p| {
        p.next += 1;
        p.waiting.push_back(Job { id: p.next, key: key.into(), message, input, done, failed });
        p.next
    });
    dispatch();
    /// The job ended early (its effect dropped): stopped.
    struct Stop(u64, bool);
    impl Drop for Stop {
        fn drop(&mut self) {
            if !self.1 {
                end(|j| j.id == self.0, None);
            }
        }
    }
    let mut stop = Stop(id, false);
    let out = JsFuture::from(p).await;
    stop.1 = true;
    match out {
        Ok(b) => Ok(b.unchecked_into::<Uint8Array>().to_vec()),
        Err(e) => Err(e.as_string().unwrap_or_else(|| format!("{e:?}"))),
    }
}

/// Stop the jobs `which` picks, waiting or running (their workers end),
/// each refused with `why` if it is said: how many.
fn end(which: impl Fn(&Job) -> bool, why: Option<&str>) -> usize {
    let ended: Vec<Job> = POOL.with_borrow_mut(|p| {
        let (gone, kept): (Vec<_>, Vec<_>) = p.waiting.drain(..).partition(|j| which(j));
        p.waiting = kept.into();
        let (stopped, running): (Vec<_>, Vec<_>) = p.running.drain(..).partition(|(j, _)| which(j));
        p.running = running;
        gone.into_iter()
            .chain(stopped.into_iter().map(|(j, w)| {
                w.terminate();
                j
            }))
            .collect()
    });
    if let Some(why) = why {
        for j in &ended {
            _ = j.failed.call1(&JsValue::NULL, &why.into());
        }
    }
    dispatch();
    ended.len()
}

/// Stop the work under `key` (`aio::effect::cancel_blocking`): how many.
pub(crate) fn cancel(key: &str) -> usize {
    end(|j| j.key == key, Some(&format!("blocking `{key}` was cancelled")))
}

/// Start waiting jobs while workers are free.
fn dispatch() {
    let most = most();
    while let Some((job, idle)) = POOL.with_borrow_mut(|p| {
        let free = !p.idle.is_empty() || p.running.len() < most;
        free.then(|| p.waiting.pop_front()).flatten().map(|j| (j, p.idle.pop()))
    }) {
        let w = match idle.map_or_else(spawn, Ok) {
            Ok(w) => w,
            Err(e) => {
                _ = job.failed.call1(&JsValue::NULL, &e.into());
                continue;
            }
        };
        let posted = w.post_message_with_transfer(&job.message, &Array::of1(&job.input.buffer()));
        POOL.with_borrow_mut(|p| p.running.push((job, w.clone())));
        if let Err(e) = posted {
            ended(&w, Err(format!("the worker took no work: {e:?}").into()));
        }
    }
}

/// A new worker on `worker.js`, its answers to [`ended`].
fn spawn() -> Result<web_sys::Worker, String> {
    let opts = web_sys::WorkerOptions::new();
    opts.set_type(web_sys::WorkerType::Module);
    let url = format!("{}worker.js", super::lazy::base());
    let w = web_sys::Worker::new_with_options(&url, &opts).map_err(|e| format!("no worker ({url}): {e:?}"))?;
    let me = w.clone();
    let answer = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
        let d: Array = e.data().unchecked_into();
        ended(&me, if d.get(0).is_truthy() { Ok(d.get(1)) } else { Err(d.get(1)) });
    });
    let me = w.clone();
    let broke = Closure::<dyn Fn(web_sys::Event)>::new(move |e: web_sys::Event| {
        let why = Reflect::get(&e, &"message".into()).ok().and_then(|m| m.as_string());
        ended(&me, Err(format!("the worker failed: {}", why.unwrap_or(format!("{url} did not run"))).into()));
    });
    w.set_onmessage(Some(answer.as_ref().unchecked_ref()));
    w.set_onerror(Some(broke.as_ref().unchecked_ref()));
    answer.forget();
    broke.forget();
    Ok(w)
}

/// Worker `w`'s job ended with `out`: it is said, and the worker takes the
/// next one (or ends, if the job failed: its module may be broken).
fn ended(w: &web_sys::Worker, out: Result<JsValue, JsValue>) {
    let j = POOL.with_borrow_mut(|p| {
        let i = p.running.iter().position(|(_, x)| x == w)?;
        let (j, w) = p.running.remove(i);
        match out.is_ok() {
            true => p.idle.push(w),
            false => w.terminate(),
        }
        Some(j)
    });
    if let Some(j) = j {
        _ = match out {
            Ok(v) => j.done.call1(&JsValue::NULL, &v),
            Err(e) => j.failed.call1(&JsValue::NULL, &e),
        };
    }
    dispatch();
}

// In a worker (`worker.js`): the page's step `step` (its table index in
// this same module) on `arg`; a step's error is thrown (the page then ends
// this worker). Not a doc comment: wasm-bindgen copies those into the glue.
#[wasm_bindgen(js_name = "__aio_work")]
pub fn work(step: u32, arg: JsValue) -> JsValue {
    // SAFETY: `step` is a `Step`'s table index in this same module, as the
    // page has it: the page took it from a `Step`, and the worker runs only
    // what the page's module sends.
    let step = unsafe { std::mem::transmute::<usize, Step>(step as usize) };
    step(arg).unwrap_or_else(|e| wasm_bindgen::throw_val(e))
}

/// A step: load the chunks the page had (`slots`), a promise.
fn chunks(slots: JsValue) -> Result<JsValue, JsValue> {
    let slots: Vec<u32> = slots.unchecked_into::<Array>().iter().filter_map(|s| s.as_f64()).map(|s| s as u32).collect();
    Ok(wasm_bindgen_futures::future_to_promise(async move {
        for slot in slots {
            super::lazy::load(slot).await?;
        }
        Ok(JsValue::UNDEFINED)
    })
    .into())
}

/// A step, the chunks loaded: `f(input)` by its runner `run` (`[run, f,
/// input]`); a panic's message is kept for the page (`__aioPanic`), and
/// the trap that ends it is thrown here.
fn job(arg: JsValue) -> Result<JsValue, JsValue> {
    std::panic::set_hook(Box::new(|info| {
        _ = Reflect::set(&js_sys::global(), &"__aioPanic".into(), &format!("panicked: {info}").into());
    }));
    let a: Array = arg.unchecked_into();
    let index = |i| a.get(i).as_f64().unwrap_or_default() as usize;
    // SAFETY: `run` is a runner's table index in this same module, as the
    // page has it (its chunks loaded first): the page took it from a
    // `Runner`, and the worker runs only what the page's module sends.
    let run = unsafe { std::mem::transmute::<usize, Runner>(index(0)) };
    let out = run(index(1), &a.get(2).unchecked_into::<Uint8Array>().to_vec())?;
    Ok(Uint8Array::from(out.as_slice()).into())
}
