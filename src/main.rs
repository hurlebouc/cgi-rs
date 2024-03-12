use std::net::SocketAddr;

use cgi_rs::server::Script;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let script = Script {
        path: "/home/hubert/src/cgi-script-go/cgi-script-go".to_string(),
        root: "".to_string(),
        dir: None,
        env: Vec::new(),
        args: Vec::new(),
        inherited_env: Vec::new(),
    };

    // We create a TcpListener and bind it to 127.0.0.1:3000
    let listener = TcpListener::bind(addr).await?;

    // We start a loop to continuously accept incoming connections
    loop {
        let script = script.clone();
        let (stream, remote) = listener.accept().await?;

        // Use an adapter to access something implementing `tokio::io` traits as if they implement
        // `hyper::rt` IO traits.
        let io = TokioIo::new(stream);
        // Spawn a tokio task to serve multiple connections concurrently
        tokio::task::spawn(async move {
            // Finally, we bind the incoming connection to our `hello` service
            if let Err(err) = http1::Builder::new()
                // `service_fn` converts our function in a `Service`
                .serve_connection(io, script.service_hyper(remote))
                //.serve_connection(io, service_fn(|req| script.server(req, remote)))
                .await
            {
                println!("Error serving connection: {:?}", err);
            }
        });
    }
}
