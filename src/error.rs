use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

pub enum AppError {
    MissingId,
    BadId,
    UnavailableProvider(String),
    TooManyIds(usize),
    BadCallback,
    BadUrl,
    SetDisabled,
    Unauthorized,
    CacheUnavailable,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::MissingId => (StatusCode::BAD_REQUEST, "ID parameter is missing".to_string()),
            AppError::BadId => (StatusCode::BAD_REQUEST, "Bad id parameter".to_string()),
            AppError::UnavailableProvider(p) => {
                (StatusCode::BAD_REQUEST, format!("Unavailable provider: {p}"))
            }
            AppError::TooManyIds(max) => {
                (StatusCode::BAD_REQUEST, format!("Too many IDs, maximum is {max}"))
            }
            AppError::BadCallback => (
                StatusCode::BAD_REQUEST,
                "Bad callback parameter: must be a JavaScript function name".to_string(),
            ),
            AppError::BadUrl => (
                StatusCode::BAD_REQUEST,
                "Bad url parameter: must be an http(s) URL".to_string(),
            ),
            AppError::SetDisabled => (
                StatusCode::FORBIDDEN,
                "/set is disabled: no setToken configured".to_string(),
            ),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "Missing or bad token".to_string()),
            AppError::CacheUnavailable => {
                (StatusCode::SERVICE_UNAVAILABLE, "Cache unavailable".to_string())
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
