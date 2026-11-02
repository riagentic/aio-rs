//! A window's events as the views take them: when the views fall behind a
//! held key (a slow machine, a debug build), the key's repeats still
//! waiting are dropped for the newest one, so the cursor stops when the
//! key is let go instead of seconds later. Everything else, and every
//! repeat when the views keep up, goes through as sent.
use futures_util::{Sink, Stream};
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

/// `S`'s frames, a held key's stale repeats left out (see the module).
pub struct Fresh<S: Stream> {
    inner: S,
    queue: VecDeque<S::Item>,
    ended: bool,
}

impl<S: Stream> Fresh<S> {
    pub fn new(inner: S) -> Self {
        Self { inner, queue: VecDeque::new(), ended: false }
    }
}

// Nothing in it is pinned in place: the queue only holds finished frames.
impl<S: Stream + Unpin> Unpin for Fresh<S> {}

/// The key and element of a repeated keydown frame, if `f` is one (air's
/// frames: `air::Raw`).
fn repeat(f: &[u8]) -> Option<String> {
    let r = air::Raw::decode(f).filter(|r| r.name == "keydown" && r.repeat)?;
    Some(format!("{}|{}", r.key, r.ids.first()?))
}

impl<S, E> Stream for Fresh<S>
where
    S: Stream<Item = Result<Vec<u8>, E>> + Unpin,
{
    type Item = Result<Vec<u8>, E>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        // Everything that is ready now, to see which repeats are stale.
        while !this.ended {
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(f)) => this.queue.push_back(f),
                Poll::Ready(None) => this.ended = true,
                Poll::Pending => break,
            }
        }
        while let Some(f) = this.queue.pop_front() {
            let stale = match &f {
                Ok(b) => repeat(b).is_some_and(|k| {
                    this.queue.iter().any(|g| g.as_ref().ok().and_then(|g| repeat(g)).is_some_and(|j| j == k))
                }),
                Err(_) => false,
            };
            if !stale {
                return Poll::Ready(Some(f));
            }
        }
        if this.ended { Poll::Ready(None) } else { Poll::Pending }
    }
}

impl<S: Stream + Sink<I> + Unpin, I> Sink<I> for Fresh<S> {
    type Error = S::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: I) -> Result<(), Self::Error> {
        Pin::new(&mut self.get_mut().inner).start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_close(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    fn key(name: &str, key: &str, repeat: bool) -> Result<Vec<u8>, ()> {
        let raw = air::Raw { ids: vec![3], name: name.into(), key: key.into(), repeat, ..air::Raw::default() };
        Ok(raw.encode())
    }

    #[tokio::test]
    async fn a_held_keys_stale_repeats_go_and_the_rest_stays_in_order() {
        let sent = vec![
            key("keydown", "ArrowDown", false),
            key("keydown", "ArrowDown", true),
            key("keydown", "ArrowDown", true),
            key("keydown", "ArrowUp", true),
            key("keydown", "ArrowDown", true),
            key("keyup", "ArrowDown", false),
            Err(()),
        ];
        let got: Vec<_> = Fresh::new(futures_util::stream::iter(sent.clone())).collect().await;
        assert_eq!(
            got,
            [&sent[0], &sent[3], &sent[4], &sent[5], &sent[6]].map(Clone::clone),
            "the first press, the newest repeats"
        );
    }

    #[test]
    fn repeats_the_views_keep_up_with_all_go_through() {
        use futures_util::FutureExt;
        let q = std::rc::Rc::new(std::cell::RefCell::new(VecDeque::new()));
        let from = q.clone();
        let mut s = Fresh::new(futures_util::stream::poll_fn(move |_| match from.borrow_mut().pop_front() {
            Some(f) => Poll::Ready(Some(f)),
            None => Poll::Pending,
        }));
        for _ in 0..3 {
            q.borrow_mut().push_back(key("keydown", "ArrowDown", true));
            assert!(s.next().now_or_never().flatten().is_some(), "one at a time: none is stale");
        }
    }
}
