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
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    input_buffer: Option<Bytes>,
    input_closed: bool,
    output_buffer_size: usize,
    child: Child, // keep reference to child process in order not to drop it before dropping the ProcessStream
}

struct PSStatus {
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    input_buffer: Option<Bytes>,
    input_closed: bool,
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
            stdout: Some(child.stdout.take().expect("Child stdout must be piped")),
            stderr: Some(child.stderr.take().expect("Child stderr must be piped")),
            input_buffer: None,
            input_closed: false,
            output_buffer_size,
            child,
        }
    }
}

impl PSStatus {
    fn next_step(
        &mut self,
        cx: &mut Context<'_>,
        output_buffer_size: usize,
    ) -> Poll<Option<Result<Output, ProcessError>>> {
        let mut buf_vec = vec![0; output_buffer_size];
        let mut readbuf = ReadBuf::new(&mut buf_vec);
        match &mut self.stdout {
            Some(stdout) => match Pin::new(stdout).poll_read(cx, &mut readbuf) {
                Poll::Ready(Ok(())) => {
                    if readbuf.filled().len() != 0 {
                        Poll::Ready(Some(Ok(Output::Stdout(Bytes::from(
                            readbuf.filled().to_vec(),
                        )))))
                    } else {
                        self.stdout = None;
                        self.next_stderr(cx, output_buffer_size)
                    }
                }
                Poll::Ready(Err(todo)) => Poll::Ready(Some(Err(ProcessError {}))),
                Poll::Pending => self.next_stderr(cx, output_buffer_size),
            },
            None => self.next_stderr(cx, output_buffer_size),
        }
    }

    fn next_stderr(
        &mut self,
        cx: &mut Context<'_>,
        output_buffer_size: usize,
    ) -> Poll<Option<Result<Output, ProcessError>>> {
        let mut buf_vec = vec![0; output_buffer_size];
        let mut readbuf = ReadBuf::new(&mut buf_vec);
        match &mut self.stderr {
            Some(stderr) => match Pin::new(stderr).poll_read(cx, &mut readbuf) {
                Poll::Ready(Ok(())) => {
                    if readbuf.filled().len() != 0 {
                        Poll::Ready(Some(Ok(Output::Stdout(Bytes::from(
                            readbuf.filled().to_vec(),
                        )))))
                    } else {
                        self.stderr = None;
                        if self.stdout.is_none() {
                            self.stdin = None;
                            Poll::Ready(None)
                        } else {
                            todo!()
                        }
                    }
                }
                Poll::Ready(Err(todo)) => Poll::Ready(Some(Err(ProcessError {}))),
                Poll::Pending => todo!(),
            },
            None => {
                if self.stdout.is_none() {
                    self.stdin = None;
                    Poll::Ready(None)
                } else {
                    todo!()
                }
            }
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

        if let Some(mut stdin) = proj.stdin.take() {
            let new_stdin: Option<ChildStdin>;
            let poll: Poll<Option<Self::Item>>;
            if let Some(v) = proj.input_buffer.take() {
                println!("--> push_to_stdin(tampon)");
                let (p, delete_stdin) = push_to_stdin(v, &mut stdin, cx, proj.input_buffer);
                new_stdin = if delete_stdin { None } else { Some(stdin) };
                poll = p;
            } else if !*proj.input_closed {
                let input_poll = proj.input.poll_next(cx);
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
