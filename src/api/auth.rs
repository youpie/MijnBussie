use std::collections::HashMap;

use axum::{extract::Request, middleware::Next, response::Response};
use dotenvy::var;
use reqwest::StatusCode;
use tracing::error;

pub async fn check_api_key(req: Request, next: Next) -> Result<Response, StatusCode> {
    let params = if let Some(query) = req.uri().query() {
        // Parse it into key-value pairs
        let params: HashMap<_, _> = url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();

        params
    } else {
        HashMap::new()
    };

    // requires the http crate to get the header name
    let api_key = var("API_KEY").unwrap_or_default();
    if params
        .get("key")
        .is_none_or(|request_key| request_key != &api_key)
    {
        error!("Denied request for incorrect key");
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(req).await)
}
