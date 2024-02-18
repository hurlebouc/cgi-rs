use std::{
    ops::{Deref, DerefMut},
    pin::{self, Pin},
    task::{Context, Poll},
};

use futures::{FutureExt, Stream, StreamExt};
use pin_project::pin_project;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    process::{ChildStdin, ChildStdout},
};

#[pin_project]
struct ProcessStream<I> {
    #[pin]
    input: I,
    #[pin]
    stdin: ChildStdin,
    #[pin]
    stdout: ChildStdout,
    tampon: Option<Vec<u8>>,
}

struct ProcessError {}

impl<I> ProcessStream<I> {
    fn get_stdout(self: Pin<&mut Self>) -> Pin<&mut ChildStdout> {
        unsafe { self.map_unchecked_mut(|s| &mut s.stdout) }
    }

    fn get_stdin(self: Pin<&mut Self>) -> Pin<&mut ChildStdin> {
        unsafe { self.map_unchecked_mut(|s| &mut s.stdin) }
    }

    fn get_input(self: Pin<&mut Self>) -> Pin<&mut I> {
        unsafe { self.map_unchecked_mut(|s| &mut s.input) }
    }
}
//impl<I> Unpin for ProcessStream<I> {}

impl<I, E> Stream for ProcessStream<I>
where
    I: Stream<Item = Result<Vec<u8>, E>>,
{
    type Item = Result<Vec<u8>, ProcessError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let proj = self.project();
        let input = proj.input;
        let stdin = proj.stdin;
        let stdout = proj.stdout;
        let buf = &mut [0; 1024];
        let mut readbuf = ReadBuf::new(buf);
        match stdout.poll_read(cx, &mut readbuf) {
            Poll::Ready(Ok(())) => Poll::Ready(Some(Ok(readbuf.filled().to_vec()))),
            Poll::Ready(Err(_)) => Poll::Ready(Some(Err(ProcessError {}))), //todo
            Poll::Pending => match proj.tampon.take() {
                Some(v) => push_to_stdin(v, stdin, cx, proj.tampon),
                None => match input.poll_next(cx) {
                    Poll::Ready(Some(Ok(v))) => push_to_stdin(v, stdin, cx, proj.tampon),
                    Poll::Ready(Some(Err(_))) => Poll::Ready(Some(Err(ProcessError {}))), //todo
                    Poll::Ready(None) => todo!(), // todo renvoyer un pending et ne plus tirer dessus
                    Poll::Pending => Poll::Pending,
                },
            },
        }
    }
}

fn push_to_stdin(
    mut v: Vec<u8>,
    stdin: Pin<&mut ChildStdin>,
    cx: &mut Context,
    tampon: &mut Option<Vec<u8>>,
) -> Poll<Option<Result<Vec<u8>, ProcessError>>> {
    match stdin.poll_write(cx, &mut v) {
        Poll::Ready(Ok(size)) => {
            if size < v.len() {
                *tampon = Some(v[size..].to_vec());
            }
            Poll::Pending
        }
        Poll::Ready(Err(_)) => Poll::Ready(Some(Err(ProcessError {}))), //todo
        Poll::Pending => Poll::Pending,
    }
}
