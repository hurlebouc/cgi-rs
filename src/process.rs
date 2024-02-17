use std::{
    ops::{Deref, DerefMut},
    pin::{self, Pin},
    task::{Context, Poll},
};

use futures::{FutureExt, Stream, StreamExt};
use pin_project::pin_project;
use tokio::{
    io::{AsyncRead, AsyncReadExt, ReadBuf},
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

    // QUESTION : que faire lorsque le processus ne lie pas les entrées qu'on lui donne ? Il va y avoir une explosion de la mémoire...

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
                Some(v) => todo!(),
                None => match input.poll_next(cx) {
                    Poll::Ready(Some(Ok(v))) => todo!(),
                    Poll::Ready(Some(Err(_))) => Poll::Ready(Some(Err(ProcessError {}))), //todo,
                    Poll::Ready(None) => todo!(),
                    Poll::Pending => todo!(),
                },
            },
        }
    }
}
