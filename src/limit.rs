use std::{
    pin::Pin,
    task::{Context, Poll},
};

use hyper::body::{Body, Frame};
use pin_project::pin_project;
use tokio::sync::OwnedSemaphorePermit;
use tokio_util::sync::PollSemaphore;

#[pin_project]
pub struct PermittedBody<B> {
    permit: Option<OwnedSemaphorePermit>,
    #[pin]
    body: B,
}

impl<B> PermittedBody<B> {
    pub fn new(permit: OwnedSemaphorePermit, body: B) -> PermittedBody<B> {
        PermittedBody {
            permit: Some(permit),
            body,
        }
    }
}

impl<B: Body> Body for PermittedBody<B> {
    type Data = B::Data;

    type Error = B::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.as_mut().project().permit.take() {
            Some(permit) => match self.as_mut().project().body.poll_frame(cx) {
                Poll::Ready(None) => Poll::Ready(None),
                poll => {
                    *self.as_mut().project().permit = Some(permit);
                    poll
                }
            },
            None => Poll::Ready(None),
        }
    }
}

pub struct HttpConcurrencyLimit<T> {
    inner: T,
    semaphore: PollSemaphore,
    /// The currently acquired semaphore permit, if there is sufficient
    /// concurrency to send a new request.
    ///
    /// The permit is acquired in `poll_ready`, and taken in `call` when sending
    /// a new request.
    permit: Option<OwnedSemaphorePermit>,
}
