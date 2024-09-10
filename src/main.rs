use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Body, Bytes, Frame};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use std::sync::RwLock;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_metrics::RuntimeMetrics;

/// This is our service handler. It receives a Request, routes on its
/// path, and returns a Future of a Response.
async fn echo(
    req: Request<hyper::body::Incoming>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    match (req.method(), req.uri().path()) {
        // Serve some instructions at /
        (&Method::GET, "/") => Ok(Response::new(full("Hello, world!"))),

        // Simply echo the body back to the client.
        (&Method::POST, "/echo") => Ok(Response::new(req.into_body().boxed())),

        // Convert to uppercase before sending back to client using a stream.
        (&Method::POST, "/echo/uppercase") => {
            let frame_stream = req.into_body().map_frame(|frame| {
                let frame = if let Ok(data) = frame.into_data() {
                    data.iter()
                        .map(|byte| byte.to_ascii_uppercase())
                        .collect::<Bytes>()
                } else {
                    Bytes::new()
                };

                Frame::data(frame)
            });

            Ok(Response::new(frame_stream.boxed()))
        }

        // Reverse the entire body before sending back to the client.
        //
        // Since we don't know the end yet, we can't simply stream
        // the chunks as they arrive as we did with the above uppercase endpoint.
        // So here we do `.await` on the future, waiting on concatenating the full body,
        // then afterwards the content can be reversed. Only then can we return a `Response`.
        (&Method::POST, "/echo/reversed") => {
            // To protect our server, reject requests with bodies larger than
            // 64kbs of data.
            // let max = req.body().size_hint().upper().unwrap_or(u64::MAX);
            // if max > 1024 * 64 {
            //     let mut resp = Response::new(full("Body too big"));
            //     *resp.status_mut() = hyper::StatusCode::PAYLOAD_TOO_LARGE;
            //     return Ok(resp);
            // }

            let whole_body = req.collect().await?.to_bytes();

            let reversed_body = whole_body.iter().rev().cloned().collect::<Vec<u8>>();
            Ok(Response::new(full(reversed_body)))
        }

        // Return the 404 Not Found for other routes.
        _ => {
            let mut not_found = Response::new(empty());
            *not_found.status_mut() = StatusCode::NOT_FOUND;
            Ok(not_found)
        }
    }
}

fn empty() -> BoxBody<Bytes, hyper::Error> {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

fn full<T: Into<Bytes>>(chunk: T) -> BoxBody<Bytes, hyper::Error> {
    Full::new(chunk.into())
        .map_err(|never| match never {})
        .boxed()
}

#[derive(Debug, Default)]
struct Metrics {
    total_steal_operations: u64,
    total_steal_count: u64,
    max_steal_operations: u64,
    max_steal_count: u64,
    num_remote_schedules: u64,
    total_local_schedule_count: u64,
    total_busy_duration: Duration,
    total_polls_count: u64,
    total_park_count: u64,
    total_overflow_count: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    let handle = tokio::runtime::Handle::current();
    let runtime_monitor = tokio_metrics::RuntimeMonitor::new(&handle);

    {
        tokio::spawn(async move {
            let mut metrics = Metrics::default();

            for interval in runtime_monitor.intervals() {
                metrics.total_steal_operations += interval.total_steal_operations;
                metrics.total_steal_count += interval.total_steal_count;
                metrics.max_steal_operations = metrics
                    .max_steal_operations
                    .max(interval.max_steal_operations);
                metrics.max_steal_count = metrics.max_steal_count.max(interval.max_steal_count);
                metrics.num_remote_schedules += interval.num_remote_schedules;
                metrics.total_local_schedule_count += interval.total_local_schedule_count;
                metrics.total_busy_duration += interval.total_busy_duration;
                metrics.total_polls_count += interval.total_polls_count;
                metrics.total_park_count += interval.total_park_count;
                metrics.total_overflow_count += interval.total_overflow_count;

                println!("{:?}", metrics);
                // wait 500ms
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        });
    }

    let listener = TcpListener::bind(addr).await?;
    println!("Listening on http://{}", addr);
    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);

        tokio::task::spawn(async move {
            if let Err(err) = http1::Builder::new()
                .serve_connection(io, service_fn(echo))
                .await
            {
                println!("Error serving connection: {:?}", err);
            }
        });
    }
}
