//! Async work of the views: tasks owned by a view (cancelled when it
//! ends) on a small executor of this thread's own, and resources (a value
//! a future loads). No threads, no dependency: the host runs the ready
//! tasks ([`run_tasks`]) when it is woken ([`on_wake`]); a task's timers
//! are the host's (tokio's, under aio-rs), polled in its context.
use crate::reactive::{
    Kind, Owner, Signal, cleanup, current_owner, current_root, effect_of, hook, untrack, with_owner,
};
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Wake, Waker};

type Fut = Pin<Box<dyn Future<Output = ()>>>;

struct Slot {
    ver: u32,
    fut: Option<Fut>,
    owner: Option<Owner>,
    /// The root current when it was made: an ownerless task ends with it.
    root: u32,
    /// Being polled (its future is out of the slot).
    polling: bool,
    /// Cancelled while polled: dropped when it comes back.
    cancelled: bool,
}

#[derive(Default)]
struct Shared {
    ready: Mutex<Vec<(u32, u32)>>,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

#[derive(Default)]
struct Exec {
    slots: Vec<Slot>,
    free: Vec<u32>,
    shared: Arc<Shared>,
}

thread_local!(static EXEC: RefCell<Exec> = RefCell::default());

struct TaskWaker {
    id: (u32, u32),
    shared: Arc<Shared>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref()
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.shared.ready.lock().unwrap_or_else(|e| e.into_inner()).push(self.id);
        let f = self.shared.wake.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(f) = f {
            f()
        }
    }
}

/// A task of the views: cancel it, or let its view's end do it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Task {
    idx: u32,
    ver: u32,
}

impl Task {
    /// Stop it now (its future is dropped).
    pub fn cancel(self) {
        cancel(self.idx, self.ver)
    }

    /// Is it still running?
    pub fn alive(self) -> bool {
        EXEC.with_borrow(|x| x.slots.get(self.idx as usize).is_some_and(|s| s.ver == self.ver && !s.cancelled))
    }
}

fn cancel(idx: u32, ver: u32) {
    let fut = EXEC.with_borrow_mut(|x| {
        let s = x.slots.get_mut(idx as usize).filter(|s| s.ver == ver)?;
        if s.polling {
            s.cancelled = true;
            return None;
        }
        let f = s.fut.take();
        s.ver = s.ver.wrapping_add(1);
        x.free.push(idx);
        f
    });
    drop(fut);
}

fn add(fut: Fut, owner: Option<Owner>) -> Task {
    let root = current_root();
    let (idx, ver, shared) = EXEC.with_borrow_mut(|x| {
        let slot = |ver| Slot { ver, fut: Some(fut), owner, root, polling: false, cancelled: false };
        let idx = match x.free.pop() {
            Some(i) => {
                let v = x.slots[i as usize].ver;
                x.slots[i as usize] = slot(v);
                i
            }
            None => {
                x.slots.push(slot(0));
                x.slots.len() as u32 - 1
            }
        };
        (idx, x.slots[idx as usize].ver, x.shared.clone())
    });
    shared.ready.lock().unwrap_or_else(|e| e.into_inner()).push((idx, ver));
    if let Some(f) = shared.wake.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        f()
    }
    Task { idx, ver }
}

/// Run `fut` for the current owner's life (a view's, a handler's, an
/// effect's): it ends with it. Outside the views it lives for the thread.
pub fn spawn(fut: impl Future<Output = ()> + 'static) -> Task {
    let t = add(Box::pin(fut), current_owner());
    let (idx, ver) = (t.idx, t.ver);
    cleanup(move || cancel(idx, ver));
    t
}

/// Run `fut` until it ends, whatever view ends first: it ends with its
/// app (the root current when it was spawned), or the thread outside any.
pub fn spawn_forever(fut: impl Future<Output = ()> + 'static) -> Task {
    add(Box::pin(fut), None)
}

/// Work for the view's life (a hook): `f()`'s future starts once the view
/// is drawn and is cancelled when it goes away.
#[track_caller]
pub fn task<F: Future + 'static>(f: impl FnOnce() -> F + 'static) {
    hook("task", || {
        let run = f();
        spawn(async move { _ = run.await });
    })
}

/// Call `f` when a task becomes ready to run (from any thread): the
/// host then calls [`run_tasks`] on this one.
pub fn on_wake(f: impl Fn() + Send + Sync + 'static) {
    let shared = EXEC.with_borrow(|x| x.shared.clone());
    *shared.wake.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
}

/// Poll every task that is ready, until none is (each under its owner, in
/// a batch: what it writes is drawn once). How many polls ran.
pub fn run_tasks() -> usize {
    let shared = EXEC.with_borrow(|x| x.shared.clone());
    let mut polls = 0;
    loop {
        let ready = std::mem::take(&mut *shared.ready.lock().unwrap_or_else(|e| e.into_inner()));
        if ready.is_empty() {
            return polls;
        }
        for (idx, ver) in ready {
            polls += poll(idx, ver, &shared, false) as usize;
        }
    }
}

/// `inline`: in the running effect, its reads followed (a resource's first poll).
fn poll(idx: u32, ver: u32, shared: &Arc<Shared>, inline: bool) -> bool {
    let got = EXEC.with_borrow_mut(|x| {
        let s = x.slots.get_mut(idx as usize).filter(|s| s.ver == ver && !s.polling)?;
        let f = s.fut.take()?;
        s.polling = true;
        Some((f, s.owner))
    });
    let Some((mut fut, owner)) = got else { return false };
    let waker = Waker::from(Arc::new(TaskWaker { id: (idx, ver), shared: shared.clone() }));
    // Put back (or dropped, if done or cancelled meanwhile) even if it panics.
    struct Back {
        idx: u32,
        fut: Option<Fut>,
        done: bool,
    }
    impl Drop for Back {
        fn drop(&mut self) {
            let fut = self.fut.take();
            let drop_it = EXEC.with_borrow_mut(|x| {
                let s = &mut x.slots[self.idx as usize];
                s.polling = false;
                if self.done || s.cancelled {
                    s.ver = s.ver.wrapping_add(1);
                    s.cancelled = false;
                    s.owner = None;
                    x.free.push(self.idx);
                    fut
                } else {
                    s.fut = fut;
                    None
                }
            });
            drop(drop_it);
        }
    }
    let mut back = Back { idx, fut: None, done: false };
    let run = || crate::reactive::batch(|| untrack(|| fut.as_mut().poll(&mut Context::from_waker(&waker)).is_ready()));
    back.done = match owner {
        _ if inline => fut.as_mut().poll(&mut Context::from_waker(&waker)).is_ready(),
        Some(o) if o.alive() => with_owner(o, run),
        Some(_) => true,
        None => crate::reactive::detached(run),
    };
    back.fut = Some(fut);
    true
}

/// A root ends: so do the ownerless tasks spawned in it (the views' end
/// cancelled the others), as their app is gone.
pub(crate) fn end_root(root: u32) {
    let gone: Vec<(u32, u32)> = EXEC.with_borrow(|x| {
        let live = |s: &Slot| s.owner.is_none() && s.root == root && (s.fut.is_some() || s.polling);
        x.slots.iter().zip(0..).filter(|(s, _)| live(s)).map(|(s, i)| (i, s.ver)).collect()
    });
    for (idx, ver) in gone {
        cancel(idx, ver)
    }
}

/// Tasks not ended on this thread (a leak shows here).
pub fn live_tasks() -> usize {
    EXEC.with_borrow(|x| x.slots.iter().filter(|s| s.fut.is_some() || s.polling).count())
}

// -------------------------------------------------------------- resources

/// A value loaded by a future: `f()` runs when the view is made and again
/// when a signal it read (before its future's first `.await`) changes, or
/// on [`restart`](Self::restart).
pub struct Resource<T> {
    value: Signal<Option<T>>,
    pending: Signal<bool>,
    again: Signal<u64>,
    off: Signal<bool>,
}

impl<T> Clone for Resource<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Resource<T> {}

impl<T> PartialEq for Resource<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T: 'static> Resource<T> {
    /// The value once loaded, cloned; subscribes.
    pub fn get(&self) -> Option<T>
    where
        T: Clone,
    {
        self.value.get()
    }

    pub fn with<R>(&self, f: impl FnOnce(Option<&T>) -> R) -> R {
        self.value.with(|v| f(v.as_ref()))
    }

    /// Is it loading? Subscribes.
    pub fn pending(&self) -> bool {
        self.pending.get()
    }

    /// Load it again.
    pub fn restart(&self) {
        self.again.update(|n| *n += 1)
    }

    /// Show `v` now, as if loaded (aio's `mutate`: an optimistic value);
    /// the next load replaces it.
    pub fn mutate(&self, v: T) {
        crate::reactive::batch(|| {
            self.value.update(|s| *s = Some(v));
            self.pending.set_quiet(false);
        })
    }

    /// Load no more (aio's `dispose`): a load under way is dropped, the
    /// value stays as it is.
    pub fn dispose(&self) {
        self.off.set(true);
    }

    /// The value, or the view stops here until it is loaded (its nearest
    /// `Suspense` shows its fallback, then the view's body runs again):
    /// `let v = res.suspend()?;`.
    pub fn suspend(&self) -> Result<T, crate::dom::ViewError>
    where
        T: Clone,
    {
        match self.value.get_untracked() {
            Some(v) => Ok(v),
            None => Err(crate::dom::ViewError::suspended(self.pending)),
        }
    }
}

/// A resource made now, under the current owner (not a hook).
pub(crate) fn resource_now<T: 'static, F: Future<Output = T> + 'static>(
    mut f: impl FnMut() -> F + 'static,
) -> Resource<T> {
    let (value, pending, again, off) = (Signal::new(None), Signal::new(true), Signal::new(0u64), Signal::new(false));
    effect_of(Kind::Effect, move || {
        if off.get() {
            return pending.set_quiet(false);
        }
        again.track();
        let fut = f();
        pending.set_quiet(true);
        let load = async move {
            let v = fut.await;
            crate::reactive::batch(|| {
                if value.alive() && !off.get_untracked() {
                    value.update(|s| *s = Some(v));
                    pending.set_quiet(false);
                }
            })
        };
        // Its first poll now, so what it reads before its first `.await` is followed.
        let t = spawn(load);
        let shared = EXEC.with_borrow(|x| x.shared.clone());
        poll(t.idx, t.ver, &shared, true);
    });
    Resource { value, pending, again, off }
}

/// A value loaded by `f()`'s future (a hook).
#[track_caller]
pub fn resource<T: 'static, F: Future<Output = T> + 'static>(f: impl FnMut() -> F + 'static) -> Resource<T> {
    hook("resource", || resource_now(f))
}
