use std::{
    pin::Pin,
    task::{Context, Poll},
};

use bytes::{Buf, Bytes};
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
    tampon: Option<Bytes>,
    input_closed: bool,
    stdin_closed: bool,
    stdout_closed: bool,
    stderr_closed: bool,
    child: Child, // keep reference to child process in order not to drop it before dropping the ProcessStream
}

pub enum Output {
    Stdout(Bytes),
    Stderr(Bytes),
}

impl Output {
    pub fn unwrap_out(self) -> Bytes {
        match self {
            Output::Stdout(v) => v,
            Output::Stderr(_) => panic!("Output is err"),
        }
    }

    pub fn unwrap_err(self) -> Bytes {
        match self {
            Output::Stderr(v) => v,
            Output::Stdout(_) => panic!("Output is out"),
        }
    }
}

#[derive(Debug)]
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
            stdin_closed: false,
            stdout_closed: false,
            stderr_closed: false,
            child,
        }
    }
}

impl<I, E> Stream for ProcessStream<I>
where
    I: Stream<Item = Result<Bytes, E>>,
{
    type Item = Result<Output, ProcessError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        //println!("poll_next");
        let proj = self.project();

        if *proj.stdout_closed && *proj.stderr_closed {
            return Poll::Ready(None);
        }

        let input = proj.input;
        let stdin = proj.stdin;
        let stdout = proj.stdout;
        let stderr = proj.stderr;
        let buf = &mut [0; 1024];
        let mut readbuf = ReadBuf::new(buf);

        if !*proj.stdout_closed {
            let stdout_poll = stdout.poll_read(cx, &mut readbuf);

            if let Poll::Ready(Ok(())) = stdout_poll {
                if readbuf.filled().len() == 0 {
                    *proj.stdout_closed = true;
                    if *proj.stderr_closed {
                        return Poll::Ready(None);
                    }
                }
                return Poll::Ready(Some(Ok(Output::Stdout(Bytes::from(
                    readbuf.filled().to_vec(),
                )))));
            }
            if let Poll::Ready(Err(_)) = stdout_poll {
                *proj.stdout_closed = true;
                *proj.stderr_closed = true;
                return Poll::Ready(Some(Err(ProcessError {}))); //todo
            }
        }

        if !*proj.stderr_closed {
            let stderr_poll = stderr.poll_read(cx, &mut readbuf);

            if let Poll::Ready(Ok(())) = stderr_poll {
                if readbuf.filled().len() == 0 {
                    *proj.stderr_closed = true;
                    if *proj.stdout_closed {
                        return Poll::Ready(None);
                    }
                }
                return Poll::Ready(Some(Ok(Output::Stderr(Bytes::from(
                    readbuf.filled().to_vec(),
                )))));
            }
            if let Poll::Ready(Err(_)) = stderr_poll {
                *proj.stdout_closed = true;
                *proj.stderr_closed = true;
                return Poll::Ready(Some(Err(ProcessError {}))); //todo
            }
        }

        if *proj.stdin_closed {
            return Poll::Pending;
        }

        if let Some(v) = proj.tampon.take() {
            return push_to_stdin(v, stdin, cx, proj.tampon, proj.stdin_closed);
        }

        if *proj.input_closed {
            return Poll::Pending;
        }

        let input_poll = input.poll_next(cx);
        if let Poll::Ready(Some(Ok(v))) = input_poll {
            return push_to_stdin(v, stdin, cx, proj.tampon, proj.stdin_closed);
        }
        if let Poll::Ready(Some(Err(_))) = input_poll {
            *proj.input_closed = true;
            return Poll::Ready(Some(Err(ProcessError {})));
        }
        if let Poll::Ready(None) = input_poll {
            *proj.input_closed = true;
            return Poll::Pending;
        }

        Poll::Pending
    }
}

fn push_to_stdin<O>(
    mut v: Bytes,
    stdin: Pin<&mut ChildStdin>,
    cx: &mut Context,
    tampon: &mut Option<Bytes>,
    stdin_closed: &mut bool,
) -> Poll<Option<Result<O, ProcessError>>> {
    match stdin.poll_write(cx, &mut v) {
        Poll::Ready(Ok(size)) => {
            if size == 0 {
                *stdin_closed = true;
            }
            if size < v.len() {
                //*tampon = Some(v[size..].to_vec());
                *tampon = Some(v.slice(size..));
            }
            Poll::Pending
        }
        Poll::Ready(Err(_)) => {
            *stdin_closed = true;
            Poll::Ready(Some(Err(ProcessError {}))) //todo
        }
        Poll::Pending => {
            *tampon = Some(v);
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod process_stream_test {
    use std::process::Stdio;

    use bytes::Bytes;
    use futures::{
        stream::{once, Once},
        StreamExt,
    };
    use tokio::process::Command;

    use super::ProcessStream;

    #[tokio::test]
    async fn simple_process_test() {
        let child = Command::new("echo")
            .arg("hello")
            .arg("world")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn");
        let input = once(async { Ok::<Bytes, String>(Bytes::from("value".as_bytes())) });
        let process_stream = ProcessStream::new(child, input);
        let s = process_stream
            .map(|r| r.unwrap().unwrap_out())
            .fold("".to_string(), |s, b| async move {
                s + &String::from_utf8_lossy(&b)
            })
            .await;
        assert_eq!(s, "hello world\n")
    }

    #[tokio::test]
    async fn read_input_test() {
        let child = Command::new("echo")
            .arg("hello")
            .arg("world")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn");
        let input = once(async { Ok::<Bytes, String>(Bytes::from("value".as_bytes())) });
        let process_stream = ProcessStream::new(child, input);
        process_stream
            .for_each(|r| async move {
                //println!("coucou");
                let b = r.unwrap().unwrap_out();
                let s = String::from_utf8(b.to_vec()).unwrap();
                print!("{}", s)
            })
            .await;
    }
}
