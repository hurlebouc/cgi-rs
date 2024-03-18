use std::{
    pin::Pin,
    task::{ready, Context, Poll},
    time::Duration,
};

use futures::Future;
use hyper::{body::Body, Request};
use pin_project::pin_project;
use tokio::time::{sleep, Sleep};
use tower::{BoxError, Layer, Service};

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

impl<B> TimeoutBody<B> {
    /// Creates a new [`TimeoutBody`].
    pub fn new(timeout: Duration, body: B) -> Self {
        TimeoutBody {
            timeout,
            sleep: None,
            body,
        }
    }
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

/// Applies a [`TimeoutBody`] to the request body.
#[derive(Clone, Debug)]
pub struct RequestBodyTimeout<S> {
    inner: S,
    timeout: Duration,
}

impl<S> RequestBodyTimeout<S> {
    /// Creates a new [`RequestBodyTimeout`].
    pub fn new(service: S, timeout: Duration) -> Self {
        Self {
            inner: service,
            timeout,
        }
    }
}

impl<S, ReqBody> Service<Request<ReqBody>> for RequestBodyTimeout<S>
where
    S: Service<Request<TimeoutBody<ReqBody>>>,
    S::Error: Into<Box<dyn std::error::Error>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let req = req.map(|body| TimeoutBody::new(self.timeout, body));
        self.inner.call(req)
    }
}

/// Applies a [`TimeoutBody`] to the request body.
#[derive(Clone, Debug)]
pub struct RequestBodyTimeoutLayer {
    timeout: Duration,
}

impl RequestBodyTimeoutLayer {
    /// Creates a new [`RequestBodyTimeoutLayer`].
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl<S> Layer<S> for RequestBodyTimeoutLayer {
    type Service = RequestBodyTimeout<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RequestBodyTimeout::new(inner, self.timeout)
    }
}
