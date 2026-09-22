use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

pub enum AppError {
    MissingId,
    BadId,
    UnavailableProvider(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let message = match self {
            AppError::MissingId => "ID parameter is missing".to_string(),
            AppError::BadId => "Bad id parameter".to_string(),
            AppError::UnavailableProvider(p) => format!("Unavailable provider: {p}"),
        };
        (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
    }
}
