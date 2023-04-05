mod codec;
mod framed;

use codec::HttpCodec;
use framed::HttpFramed;

use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use dirtio::net::tcp::{TcpListener, TcpStream};
use futures::{SinkExt, StreamExt};
use http::{Request, Response, StatusCode};
use log::{debug, info};

#[dirtio::main]
fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8080".to_string())
        .parse()
        .expect("invalid address");

    let mut listener = TcpListener::bind(addr)?;
    info!("listening on: {}", addr);

    loop {
        let (stream, _) = listener.accept().await?;

        dirtio::spawn(async move {
            if let Err(e) = process(stream).await {
                debug!("failed to process stream: {}", e);
            };
        });
    }
}

async fn process(stream: TcpStream) -> Result<(), Box<dyn Error>> {
    let mut transport = HttpFramed::new(stream, HttpCodec);

    while let Some(item) = transport.next().await {
        match item {
            Ok(request) => {
                let response = respond(&request)?;
                transport.send(response).await?;

                // Client may want to close connection.
                if let Some("close") = request
                    .headers()
                    .get("Connection")
                    .and_then(|v| v.to_str().ok())
                {
                    break;
                }
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }
    Ok(())
}

fn respond(req: &Request<()>) -> Result<Response<Vec<u8>>, Box<dyn Error>> {
    let mut response = Response::builder();

    // Normalize path to prevent path traversal attack.
    let mut path = req
        .uri()
        .path()
        .split('/')
        .filter(|c| !c.is_empty() && *c != "." && *c != "..")
        .fold(PathBuf::from(Path::new(".")), |mut path, c| {
            path.push(c);
            path
        });

    // Default to index.html.
    if path.is_dir() {
        path.push("index.html");
    }

    let body = match fs::read(path) {
        Ok(content) => content,
        Err(_) => {
            response = response.status(StatusCode::NOT_FOUND);
            Vec::new()
        }
    };

    Ok(response.body(body)?)
}
