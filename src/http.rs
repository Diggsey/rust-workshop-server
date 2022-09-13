use hyper::{header::CONTENT_TYPE, http::HeaderValue, Response};
use tower::make::Shared;
use tower_http::{
    services::{fs::ServeFileSystemResponseBody, ServeDir},
    set_header::SetResponseHeader,
};

fn fix_content_type(resp: &Response<ServeFileSystemResponseBody>) -> Option<HeaderValue> {
    if let Some(header_value) = resp.headers().get(CONTENT_TYPE) {
        if header_value == "video/vnd.dlna.mpeg-tts" {
            return Some(HeaderValue::from_static("application/octet-stream"));
        }
    }
    None
}

#[tokio::main]
pub async fn run_server() {
    let service =
        SetResponseHeader::overriding(ServeDir::new("static"), CONTENT_TYPE, fix_content_type);

    hyper::Server::bind(&"0.0.0.0:80".parse().unwrap())
        .serve(Shared::new(service))
        .await
        .expect("server error");
}
