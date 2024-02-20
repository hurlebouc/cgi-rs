use std::{
    pin::Pin,
    task::{Context, Poll},
};

use bytes::Bytes;
use futures::Stream;
use pin_project::pin_project;
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout},
};

#[pin_project]
pub struct ProcessStream<I> {
    #[pin]
    input: I,
    //#[pin]
    stdin: Option<ChildStdin>,
    #[pin]
    stdout: ChildStdout,
    #[pin]
    stderr: ChildStderr,
    input_buffer: Option<Bytes>,
    input_closed: bool,
    stdout_closed: bool,
    stderr_closed: bool,
    output_buffer_size: usize,
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
    pub fn new(mut child: Child, input: I, output_buffer_size: usize) -> ProcessStream<I> {
        ProcessStream {
            input,
            stdin: Some(child.stdin.take().expect("Child stdin must be piped")),
            stdout: child.stdout.take().expect("Child stdout must be piped"),
            stderr: child.stderr.take().expect("Child stderr must be piped"),
            input_buffer: None,
            input_closed: false,
            stdout_closed: false,
            stderr_closed: false,
            output_buffer_size,
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
        println!("poll_next");
        let proj = self.project();

        if *proj.stdout_closed && *proj.stderr_closed {
            println!("--> stdout and stderr closed");
            return Poll::Ready(None);
        }

        let input = proj.input;
        let stdout = proj.stdout;
        let stderr = proj.stderr;
        let mut buf_vec = vec![0; *proj.output_buffer_size];
        let mut readbuf = ReadBuf::new(&mut buf_vec);

        if !*proj.stdout_closed {
            let stdout_poll = stdout.poll_read(cx, &mut readbuf);

            if let Poll::Ready(Err(_)) = stdout_poll {
                *proj.stdout_closed = true;
                println!("--> stdout gives error");
                return Poll::Ready(Some(Err(ProcessError {}))); //todo
            }

            if let Poll::Ready(Ok(())) = stdout_poll {
                if readbuf.filled().len() != 0 {
                    println!("--> stdout gives output");
                    return Poll::Ready(Some(Ok(Output::Stdout(Bytes::from(
                        readbuf.filled().to_vec(),
                    )))));
                } else {
                    *proj.stdout_closed = true;
                    if *proj.stderr_closed {
                        println!("--> stdout closed");
                        return Poll::Ready(None);
                    }
                }
            }
            println!("--> No stdout ouput")
        }

        if !*proj.stderr_closed {
            let stderr_poll = stderr.poll_read(cx, &mut readbuf);

            if let Poll::Ready(Err(_)) = stderr_poll {
                *proj.stderr_closed = true;
                println!("--> stderr gives error");
                return Poll::Ready(Some(Err(ProcessError {}))); //todo
            }

            if let Poll::Ready(Ok(())) = stderr_poll {
                if readbuf.filled().len() != 0 {
                    println!("--> stderr gives output");
                    return Poll::Ready(Some(Ok(Output::Stderr(Bytes::from(
                        readbuf.filled().to_vec(),
                    )))));
                } else {
                    *proj.stderr_closed = true;
                    if *proj.stdout_closed {
                        println!("--> stderr closed");
                        return Poll::Ready(None);
                    }
                }
            }
            println!("--> No stderr ouput")
        }

        if let Some(mut stdin) = proj.stdin.take() {
            let new_stdin: Option<ChildStdin>;
            let poll: Poll<Option<Self::Item>>;
            if let Some(v) = proj.input_buffer.take() {
                println!("--> push_to_stdin(tampon)");
                let (p, delete_stdin) = push_to_stdin(v, &mut stdin, cx, proj.input_buffer);
                new_stdin = if delete_stdin { None } else { Some(stdin) };
                poll = p;
            } else if !*proj.input_closed {
                let input_poll = input.poll_next(cx);
                if let Poll::Ready(Some(Ok(v))) = input_poll {
                    println!("--> push_to_stdin(input)");
                    let (p, delete_stdin) = push_to_stdin(v, &mut stdin, cx, proj.input_buffer);
                    new_stdin = if delete_stdin { None } else { Some(stdin) };
                    poll = p;
                } else if let Poll::Ready(Some(Err(_))) = input_poll {
                    println!("--> input error");
                    *proj.input_closed = true;
                    new_stdin = Some(stdin);
                    poll = Poll::Ready(Some(Err(ProcessError {}))); //todo
                } else if let Poll::Pending = input_poll {
                    println!("--> Wait for input");
                    new_stdin = Some(stdin);
                    poll = Poll::Pending;
                } else if let Poll::Ready(None) = input_poll {
                    println!("--> input end");
                    *proj.input_closed = true;
                    *proj.stdin = None;
                    new_stdin = None;
                    poll = Poll::Pending;
                } else {
                    new_stdin = Some(stdin);
                    poll = Poll::Pending;
                }
            } else {
                new_stdin = Some(stdin);
                poll = Poll::Pending;
            }
            *proj.stdin = new_stdin;
            return poll;
        }

        //if *proj.input_closed && *proj.stdin_closed {
        //    println!("--> input and stdin are closed");
        //    return Poll::Pending;
        //}

        println!("--> default response");
        Poll::Pending
    }
}

fn push_to_stdin<O>(
    mut v: Bytes,
    stdin: &mut ChildStdin,
    cx: &mut Context,
    tampon: &mut Option<Bytes>,
) -> (Poll<Option<Result<O, ProcessError>>>, bool) {
    match Pin::new(stdin).poll_write(cx, &mut v) {
        Poll::Ready(Ok(size)) => {
            println!("--> stdin accept. size = {}", size);
            let delete_stdin: bool;
            if size == 0 {
                delete_stdin = true;
            } else {
                delete_stdin = false;
            }
            if size < v.len() {
                *tampon = Some(v.slice(size..));
            }
            (Poll::Pending, delete_stdin)
        }
        Poll::Ready(Err(_)) => {
            println!("--> Error while writing to stdin");
            (Poll::Ready(Some(Err(ProcessError {}))), true) //todo
        }
        Poll::Pending => {
            println!("--> stdin pending");
            *tampon = Some(v);
            (Poll::Pending, false)
        }
    }
}

#[cfg(test)]
mod process_stream_test {
    use std::process::Stdio;

    use bytes::Bytes;
    use futures::{
        stream::{self},
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
        let input = stream::empty::<Result<Bytes, String>>();
        let process_stream = ProcessStream::new(child, input, 1024);
        let s = process_stream
            .map(|r| r.unwrap().unwrap_out())
            .fold("".to_string(), |s, b| async move {
                s + &String::from_utf8_lossy(&b)
            })
            .await;
        assert_eq!(s, "hello world\n")
    }

    #[tokio::test]
    async fn small_buffer_test() {
        let child = Command::new("echo")
            .arg("hello")
            .arg("world")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn");
        let input = stream::empty::<Result<Bytes, String>>();
        let process_stream = ProcessStream::new(child, input, 1);
        let s = process_stream
            .map(|r| r.unwrap().unwrap_out())
            .fold("".to_string(), |s, b| async move {
                s + &String::from_utf8_lossy(&b)
            })
            .await;
        assert_eq!(s, "hello world\n")
    }

    //#[tokio::test(flavor = "multi_thread")]
    #[tokio::test]
    async fn read_input_test() {
        println!("read_input_test");
        let child = Command::new("cat")
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn");
        //let input = stream::empty::<Result<Bytes, String>>();
        let input = stream::once(async { Ok::<Bytes, String>(Bytes::from("value".as_bytes())) });
        let process_stream = ProcessStream::new(child, input, 1024);
        let s = process_stream
            .map(|r| r.unwrap().unwrap_out())
            .fold("".to_string(), |s, b| async move {
                println!("RES: {}", String::from_utf8_lossy(&b));
                s + &String::from_utf8_lossy(&b)
            })
            .await;
        assert_eq!(s, "value")
    }
}
