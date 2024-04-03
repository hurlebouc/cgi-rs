mod limit;
mod timeout;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;

use cgi_rs::server::Script;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use limit::GlobalHttpConcurrencyLimitLayer;
use timeout::RequestBodyTimeoutLayer;
use tokio::io::{stderr, AsyncWrite, Stderr};
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower_http::timeout::ResponseBodyTimeoutLayer;

use clap::Parser;

/// Simple CGI server
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Binding address and port (default 0.0.0.0:8080)
    #[arg(long)]
    address: Option<String>,

    /// Root of the script (default "")
    #[arg(long)]
    root: Option<PathBuf>,

    /// Directory of the cgi script
    #[arg(long)]
    dir: Option<PathBuf>,

    /// Request body read timeout in millisecond (default "30000")
    #[arg(long = "req-body-timeout")]
    request_body_timeout: Option<u64>,

    /// Response body read timeout in millisecond (default "30000")
    #[arg(long = "res-body-timeout")]
    response_body_timeout: Option<u64>,

    /// Max number of parallel processes (default "4")
    #[arg(long = "max-processes")]
    max_processes: Option<u16>,

    /// Path of cgi script
    path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();
    let binding_address = args.address.as_deref().unwrap_or("0.0.0.0:8080");
    let addr = SocketAddr::from_str(binding_address).expect(&format!(
        "Cannot parse {} as binding address",
        &binding_address
    ));
    let script = Script {
        path: args.path,
        root: args.root.unwrap_or(PathBuf::new()),
        dir: args.dir,
        env: Vec::new(),
        args: Vec::new(),
        inherited_env: Vec::new(),
    };
    //let semaphore = Arc::new(Semaphore::new(1));
    // let concurrence_layer = GlobalConcurrencyLimitLayer::new(1);
    let concurrence_layer =
        GlobalHttpConcurrencyLimitLayer::new(args.max_processes.unwrap_or(4).into());

    // We create a TcpListener and bind it to 127.0.0.1:3000
    let listener = TcpListener::bind(addr).await?;

    // We start a loop to continuously accept incoming connections
    loop {
        let script = script.clone();
        //let semaphore = semaphore.clone();
        let concurrence_layer = concurrence_layer.clone();
        let (stream, remote) = listener.accept().await?;

        // Use an adapter to access something implementing `tokio::io` traits as if they implement
        // `hyper::rt` IO traits.
        let io = TokioIo::new(stream);
        // Spawn a tokio task to serve multiple connections concurrently
        tokio::task::spawn(async move {
            let service = ServiceBuilder::new()
                .layer(concurrence_layer)
                .layer(RequestBodyTimeoutLayer::new(Duration::from_millis(
                    args.request_body_timeout.unwrap_or(30000),
                )))
                .layer(ResponseBodyTimeoutLayer::new(Duration::from_millis(
                    args.response_body_timeout.unwrap_or(30000),
                )))
                //.service_fn(handle);
                .service_fn(|req| script.serve(req, remote, ClonableStderr::new()));
            //.service(script.service(remote));
            // Finally, we bind the incoming connection to our `hello` service
            if let Err(err) = http1::Builder::new()
                // `service_fn` converts our function in a `Service`
                //.serve_connection(io, script.service_hyper(remote))
                // .serve_connection(
                //     io,
                //     service_fn(|req| async {
                //         let permit = semaphore.clone().acquire_owned().await.unwrap();
                //         script
                //             .server(req, remote)
                //             .await
                //             .map(|resp| resp.map(|body| PermittedBody::new(permit, body)))
                //     }),
                // )
                .serve_connection(io, hyper_util::service::TowerToHyperService::new(service))
                .await
            {
                println!("Error serving connection: {:?}", err);
            }
        });
    }
}

struct ClonableStderr(Stderr);

impl ClonableStderr {
    fn new() -> ClonableStderr {
        ClonableStderr(stderr())
    }
}

impl Clone for ClonableStderr {
    fn clone(&self) -> Self {
        Self(stderr())
    }
}

impl AsyncWrite for ClonableStderr {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}
