use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures::Stream;
use pin_project::pin_project;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout},
};

#[pin_project]
pub struct ProcessStream<I> {
    #[pin]
    input: I,
    #[pin]
    stdin: ChildStdin,
    #[pin]
    stdout: ChildStdout,
    #[pin]
    stderr: ChildStderr,
    tampon: Option<Vec<u8>>,
    input_closed: bool,
    child: Child, // keep reference to child process in order not to drop it before dropping the ProcessStream
}

pub enum Output {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
}

pub struct ProcessError {}

impl<I> ProcessStream<I> {
    /// Creates a new [`ProcessStream<I>`].
    ///
    /// # Panics
    ///
    /// Panics if stdin, stdout or stderr is not piped.
    pub fn new(mut child: Child, input: I) -> ProcessStream<I> {
        ProcessStream {
            input,
            stdin: child.stdin.take().expect("Child stdin must be piped"),
            stdout: child.stdout.take().expect("Child stdout must be piped"),
            stderr: child.stderr.take().expect("Child stderr must be piped"),
            tampon: None,
            input_closed: false,
            child,
        }
    }
}

impl<I, E> Stream for ProcessStream<I>
where
    I: Stream<Item = Result<Vec<u8>, E>>,
{
    type Item = Result<Output, ProcessError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let proj = self.project();
        let input = proj.input;
        let stdin = proj.stdin;
        let stdout = proj.stdout;
        let stderr = proj.stderr;
        let buf = &mut [0; 1024];
        let mut readbuf = ReadBuf::new(buf);
        match stdout.poll_read(cx, &mut readbuf) {
            Poll::Ready(Ok(())) => Poll::Ready(Some(Ok(Output::Stdout(readbuf.filled().to_vec())))),
            Poll::Ready(Err(_)) => Poll::Ready(Some(Err(ProcessError {}))), //todo
            Poll::Pending => match stderr.poll_read(cx, &mut readbuf) {
                Poll::Ready(Ok(())) => {
                    Poll::Ready(Some(Ok(Output::Stderr(readbuf.filled().to_vec()))))
                }
                Poll::Ready(Err(_)) => Poll::Ready(Some(Err(ProcessError {}))), //todo
                Poll::Pending => match proj.tampon.take() {
                    Some(v) => push_to_stdin(v, stdin, cx, proj.tampon),
                    None => {
                        if !*proj.input_closed {
                            match input.poll_next(cx) {
                                Poll::Ready(Some(Ok(v))) => {
                                    push_to_stdin(v, stdin, cx, proj.tampon)
                                }
                                Poll::Ready(Some(Err(_))) => {
                                    Poll::Ready(Some(Err(ProcessError {})))
                                } //todo
                                Poll::Ready(None) => {
                                    *proj.input_closed = true;
                                    Poll::Pending
                                }
                                Poll::Pending => Poll::Pending,
                            }
                        } else {
                            Poll::Pending
                        }
                    }
                },
            },
        }
    }
}

fn push_to_stdin<O>(
    mut v: Vec<u8>,
    stdin: Pin<&mut ChildStdin>,
    cx: &mut Context,
    tampon: &mut Option<Vec<u8>>,
) -> Poll<Option<Result<O, ProcessError>>> {
    match stdin.poll_write(cx, &mut v) {
        Poll::Ready(Ok(size)) => {
            if size < v.len() {
                *tampon = Some(v[size..].to_vec());
            }
            Poll::Pending
        }
        Poll::Ready(Err(_)) => Poll::Ready(Some(Err(ProcessError {}))), //todo
        Poll::Pending => {
            *tampon = Some(v);
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod process_stream_test {
    use std::process::Stdio;

    use tokio::process::Command;
    use tokio_stream::once;

    use super::ProcessStream;

    #[tokio::test(flavor = "multi_thread")]
    async fn simple_process_test() {
        let child = Command::new("echo")
            .arg("hello")
            .arg("world")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn");
        let input: tokio_stream::Once<Result<Vec<u8>, String>> =
            once(Ok("value".as_bytes().to_vec()));
        let process_stream = ProcessStream::new(child, input);
    }
}
