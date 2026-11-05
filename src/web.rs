//! Status page, JSON status, MJPEG stream and snapshot.
//!
//! `/stream` and `/snapshot` use the same paths as ustreamer, so clients set up for it keep working.

use std::{convert::Infallible, sync::Arc, time::Duration};

use anyhow::Result;
use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use bytes::{BufMut, Bytes, BytesMut};
use serde_json::{Value, json};
use tokio::time;
use tracing::info;

use crate::{camera, laser, net};

#[derive(Clone)]
pub struct AppState {
    pub laser: laser::Shared,
    pub camera: camera::Shared,
    pub frames: Option<camera::Frames>,
    pub hostname: Arc<str>,
}

pub async fn run(port: u16, state: AppState) -> Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/api/status", get(status))
        .route("/stream", get(stream))
        .route("/snapshot", get(snapshot))
        .with_state(state);
    let listener = net::bind_tcp(port).await?;
    info!("web: status page on port {port}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(include_str!("index.html"))
}

async fn status(State(s): State<AppState>) -> Json<Value> {
    let laser = s.laser.lock().unwrap().clone();
    let camera = s.camera.lock().unwrap().clone();
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "hostname": &*s.hostname,
        "laser": laser,
        "camera": camera,
    }))
}

fn unavailable() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, "camera not available\n").into_response()
}

async fn snapshot(State(s): State<AppState>) -> Response {
    let Some(mut rx) = s.frames.clone() else {
        return unavailable();
    };
    let frame = time::timeout(Duration::from_secs(3), async move {
        loop {
            if let Some(f) = rx.borrow_and_update().clone() {
                return Some(f);
            }
            rx.changed().await.ok()?;
        }
    })
    .await
    .ok()
    .flatten();
    match frame {
        Some(data) => (
            [
                (header::CONTENT_TYPE, "image/jpeg"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            data,
        )
            .into_response(),
        None => unavailable(),
    }
}

/// Counts open streams for the status page.
struct Viewer(camera::Shared);

impl Viewer {
    fn new(status: camera::Shared) -> Self {
        status.lock().unwrap().viewers += 1;
        Self(status)
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.0.lock().unwrap().viewers -= 1;
    }
}

async fn stream(State(s): State<AppState>) -> Response {
    let Some(mut rx) = s.frames.clone() else {
        return unavailable();
    };
    rx.mark_changed();
    let viewer = Viewer::new(s.camera.clone());
    // A slow client simply skips frames: the watch channel only keeps the newest one.
    let parts = futures_util::stream::unfold((rx, viewer), |(mut rx, viewer)| async move {
        loop {
            rx.changed().await.ok()?;
            let Some(data) = rx.borrow_and_update().clone() else {
                continue;
            };
            return Some((Ok::<Bytes, Infallible>(part(&data)), (rx, viewer)));
        }
    });
    Response::builder()
        .header(
            header::CONTENT_TYPE,
            "multipart/x-mixed-replace; boundary=frame",
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(parts))
        .unwrap()
}

fn part(jpeg: &[u8]) -> Bytes {
    let head = format!(
        "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
        jpeg.len()
    );
    let mut buf = BytesMut::with_capacity(head.len() + jpeg.len() + 2);
    buf.put_slice(head.as_bytes());
    buf.put_slice(jpeg);
    buf.put_slice(b"\r\n");
    buf.freeze()
}
