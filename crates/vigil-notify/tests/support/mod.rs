//! Helpers for the tests that talk to a mock server.

#![allow(dead_code)]

use serde_json::Value;
use vigil_notify::{Channel, Endpoints, Event, Prepared, SendError, Sender, SenderConfig, prepare};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// A server that accepts every POST.
pub async fn accepting_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    server
}

pub fn port(server: &MockServer) -> u16 {
    server.address().port()
}

/// A sender whose fixed-address services all live on `server`.
pub fn sender(server: &MockServer) -> Sender {
    let endpoints = Endpoints {
        telegram: server.uri(),
        discord: server.uri(),
        slack: server.uri(),
    };
    match Sender::new(SenderConfig {
        endpoints,
        ..SenderConfig::default()
    }) {
        Ok(sender) => sender,
        Err(error) => panic!("sender: {error}"),
    }
}

pub fn channel(url: &str) -> Channel {
    match Channel::parse(url) {
        Ok(channel) => channel,
        Err(error) => panic!("{url}: {error}"),
    }
}

pub fn prepared(channel: &Channel, event: &Event) -> Prepared {
    prepare(channel.markup(), channel.limit(), event, None)
}

/// Send the sample event to `url` once.
pub async fn send_once(server: &MockServer, url: &str) -> Result<(), SendError> {
    let channel = channel(url);
    let event = Event::sample();
    sender(server)
        .send(&channel, &prepared(&channel, &event), &event)
        .await
}

/// The one request the server received.
pub async fn only_request(server: &MockServer) -> Request {
    let mut requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(requests.len(), 1, "expected exactly one request");
    requests.remove(0)
}

pub fn json_body(request: &Request) -> Value {
    match serde_json::from_slice(&request.body) {
        Ok(value) => value,
        Err(error) => panic!("body is not JSON: {error}"),
    }
}

pub fn header(request: &Request, name: &str) -> String {
    request
        .headers
        .get(name)
        .map(|value| value.to_str().unwrap_or_default().to_owned())
        .unwrap_or_default()
}
