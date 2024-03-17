use std::{
    pin::Pin,
    task::{ready, Poll},
    time::Duration,
};

use futures::Future;
use hyper::body::Body;
use pin_project::pin_project;
use tokio::time::{sleep, Sleep};
use tower::BoxError;

/// Error for [`TimeoutBody`].
#[derive(Debug)]
pub struct TimeoutError(());

impl std::error::Error for TimeoutError {}

impl std::fmt::Display for TimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "data was not received within the designated timeout")
    }
}

#[pin_project]
pub struct TimeoutBody<B> {
    timeout: Duration,
    sleep: Option<Pin<Box<Sleep>>>,
    #[pin]
    body: B,
}

impl<B> Body for TimeoutBody<B>
where
    B: Body,
    B::Error: Into<BoxError>,
{
    type Data = B::Data;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();

        // Start the `Sleep` if not active.
        if this.sleep.is_none() {
            *this.sleep = Some(Box::pin(sleep(*this.timeout)));
        }

        let sleep_pinned = this.sleep.as_mut().map(|p| p.as_mut()).unwrap();

        // Error if the timeout has expired.
        if let Poll::Ready(()) = sleep_pinned.poll(cx) {
            return Poll::Ready(Some(Err(Box::new(TimeoutError(())))));
        }

        // Check for body data.
        let frame = ready!(this.body.poll_frame(cx));
        // A frame is ready. Reset the `Sleep`...
        *this.sleep = None;

        Poll::Ready(frame.transpose().map_err(Into::into).transpose())
    }
}
