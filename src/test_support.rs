use serenity::http::{Http, HttpBuilder};
use wiremock::{MockServer, ResponseTemplate};

pub(crate) fn json_response(status: u16, body: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_raw(body, "application/json")
}

/// A Discord HTTP client that sends its requests to `server`.
pub(crate) fn discord_http(server: &MockServer) -> Http {
    HttpBuilder::new("test-token")
        .proxy(server.uri())
        .ratelimiter_disabled(true)
        .build()
}
