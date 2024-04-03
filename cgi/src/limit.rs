use std::{
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};

use core::future::Future;
use hyper::{
    body::{Body, Frame, Incoming},
    Request, Response,
};
use pin_project::pin_project;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::PollSemaphore;
use tower::Layer;

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

pub struct HttpConcurrencyLimit<S> {
    service: S,
    semaphore: PollSemaphore,
    /// The currently acquired semaphore permit, if there is sufficient
    /// concurrency to send a new request.
    ///
    /// The permit is acquired in `poll_ready`, and taken in `call` when sending
    /// a new request.
    permit: Option<OwnedSemaphorePermit>,
}

impl<T: Clone> Clone for HttpConcurrencyLimit<T> {
    fn clone(&self) -> Self {
        // Since we hold an `OwnedSemaphorePermit`, we can't derive `Clone`.
        // Instead, when cloning the service, create a new service with the
        // same semaphore, but with the permit in the un-acquired state.
        Self {
            service: self.service.clone(),
            semaphore: self.semaphore.clone(),
            permit: None,
        }
    }
}

impl<S, B> tower::Service<Request<Incoming>> for HttpConcurrencyLimit<S>
where
    S: tower::Service<Request<Incoming>, Response = Response<B>>,
{
    type Response = Response<PermittedBody<B>>;

    type Error = S::Error;

    type Future = HttpConcurrencyLimitFut<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // If we haven't already acquired a permit from the semaphore, try to
        // acquire one first.
        if self.permit.is_none() {
            self.permit = ready!(self.semaphore.poll_acquire(cx));
            debug_assert!(
                self.permit.is_some(),
                "ConcurrencyLimit semaphore is never closed, so `poll_acquire` \
                 should never fail",
            );
        }

        // Once we've acquired a permit (or if we already had one), poll the
        // inner service.
        self.service.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Incoming>) -> Self::Future {
        // Take the permit
        let permit = self
            .permit
            .take()
            .expect("max requests in-flight; poll_ready must be called first");

        // Call the inner service
        let future = self.service.call(req);

        HttpConcurrencyLimitFut {
            future,
            permit: Some(permit),
        }
    }
}

#[pin_project]
pub struct HttpConcurrencyLimitFut<F> {
    #[pin]
    future: F,
    permit: Option<OwnedSemaphorePermit>,
}

impl<F, B, E> Future for HttpConcurrencyLimitFut<F>
where
    F: Future<Output = Result<Response<B>, E>>,
{
    type Output = Result<Response<PermittedBody<B>>, E>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.as_mut().project().future.poll(cx) {
            Poll::Ready(Ok(resp)) => match self.as_mut().project().permit.take() {
                Some(permit) => Poll::Ready(Ok(resp.map(|body| PermittedBody::new(permit, body)))),
                None => unreachable!("Future should not be polled twice"),
            },
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GlobalHttpConcurrencyLimitLayer {
    semaphore: Arc<Semaphore>,
}

impl GlobalHttpConcurrencyLimitLayer {
    /// Create a new `GlobalConcurrencyLimitLayer`.
    pub fn new(max: usize) -> Self {
        Self::with_semaphore(Arc::new(Semaphore::new(max)))
    }

    /// Create a new `GlobalConcurrencyLimitLayer` from a `Arc<Semaphore>`
    pub fn with_semaphore(semaphore: Arc<Semaphore>) -> Self {
        GlobalHttpConcurrencyLimitLayer { semaphore }
    }
}

impl<S> Layer<S> for GlobalHttpConcurrencyLimitLayer {
    type Service = HttpConcurrencyLimit<S>;

    fn layer(&self, service: S) -> Self::Service {
        HttpConcurrencyLimit {
            service,
            semaphore: PollSemaphore::new(self.semaphore.clone()),
            permit: None,
        }
    }
}
