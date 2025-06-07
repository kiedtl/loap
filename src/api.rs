use std::time::Duration;

use axum::{
    extract::{Json, Query, State},
    http::{Method, StatusCode},
    response::{Response, Redirect, IntoResponse},
};
use chrono::{DateTime, Utc};
use object_store::{ObjectStore, signer::Signer};
use serde_json;
use serde::{Serialize, Deserialize};
use sqlx::Row;
use uuid::Uuid;

use log::{info, error};

use crate::{
    AppState, AuthCheck,
    verify_maintainer, get_package_id, get_packages, get_builds_by_id,
};

pub enum AnyOf2<A, B> {
    A(A), B(B)
}

impl<A, B> IntoResponse for AnyOf2<A, B>
where
    A: IntoResponse,
    B: IntoResponse,
{
    fn into_response(self) -> Response {
        match self {
            AnyOf2::A(i) => i.into_response(),
            AnyOf2::B(i) => i.into_response(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ServerError {
    inner: String,
}

impl From<anyhow::Error> for ServerError {
    fn from(e: anyhow::Error) -> Self {
        Self { inner: format!("{e:#}") }
    }
}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        error!("handled api error: {}", self.inner);
        (StatusCode::INTERNAL_SERVER_ERROR, self.inner).into_response()
    }
}

#[derive(Copy, Clone)]
pub enum ApiError {
    NotFound,
    ReportedObjectNotFound,
    Auth(AuthCheck),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        use StatusCode as SC;

        match self {
            ApiError::NotFound => (SC::NOT_FOUND, "Not found.").into_response(),
            ApiError::ReportedObjectNotFound => (SC::NOT_FOUND, "Reported object doesn't exist, or inaccessible.").into_response(),
            ApiError::Auth(ac) => (SC::FORBIDDEN, ac.to_string()).into_response(),
        }
    }
}


#[derive(Copy, Clone, Debug, Deserialize)]
pub struct DownloadByBuildId {
    id: u32,
}

pub async fn download_by_build_id(
    State(state): State<AppState>,
    Query(args): Query<DownloadByBuildId>,
)
    -> Result<AnyOf2<Redirect, ApiError>, String>
{
    info!("api: handling download_by_build_id {:?}", args);

    let mut conn = state.db.lock().await;
    let s3 = state.s3;

    let query_result = sqlx::query("SELECT object_path FROM Builds where id = $1")
        .bind(args.id)
        .fetch_one(&mut *conn)
        .await;

    let path = match query_result {
        Ok(d) => d.try_get::<String, _>(0).map_err(|err| err.to_string())?,
        Err(sqlx::Error::RowNotFound) => return Ok(AnyOf2::B(ApiError::NotFound)),
        Err(e) => return Err(e.to_string()),
    };

    let url = s3.signed_url(
        Method::GET,
        &object_store::path::Path::from(path.clone()),
        Duration::from_secs(3 * 60)
    )
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query("UPDATE Builds SET downloads = downloads + 1 WHERE id = $1")
        .bind(args.id)
        .execute(&mut *conn)
        .await
        .map_err(|e| e.to_string())?;

    Ok(AnyOf2::A(
        Redirect::to(url.as_str())
    ))
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReportBuild {
    built_by: String,
    package: String,
    version: String,
    completed_in: u32,
    completed_at: DateTime<Utc>,
    object_path: String,
    signature: String,
}

pub async fn report_build(
    State(state): State<AppState>,
    Json(q): Json<ReportBuild>,
)
    -> Result<AnyOf2<String, ApiError>, String>
{
    info!("api: handling report_build {:?}", q);

    let mut conn = state.db.lock().await;
    let s3 = state.s3;

    let (maintainer_id, pkg_id) = match verify_maintainer(
        &mut *conn, "loap-upload-file", &q.built_by, &q.package, q.completed_at, &q.signature
    ).await? {
        AuthCheck::Ok(mi, pi) => (mi, pi),
        err => return Ok(AnyOf2::B(ApiError::Auth(err))),
    };

    let res = s3.get_opts(
        &object_store::path::Path::from(q.object_path.clone()),
        object_store::GetOptions { head: true, .. Default::default() },
    ).await;

    let size = match res {
        Ok(object_store::GetResult { meta, .. }) =>
            // FIXME avoid cast somehow? (sqlite doesn't have u64 though...)
            meta.size as u32,
        Err(_) => return Ok(AnyOf2::B(ApiError::ReportedObjectNotFound)),
    };

    let r = sqlx::query(
        "INSERT INTO Builds
            (built_by, package, version, size, completed_at, completed_in, result, downloads, object_path)
        VALUES
            ($1, $2, $3, $4, $5, $6, 0, 0, $7);"
    )
        .bind(maintainer_id)
        .bind(pkg_id)
        .bind(q.version)
        .bind(size)
        .bind(q.completed_at.timestamp())
        .bind(q.completed_in)
        .bind(q.object_path)
        .execute(&mut *conn)
        .await
        .map_err(|err| err.to_string())?;

    if r.rows_affected() != 1 {
        Err("Internal database error".to_string())
    } else {
        Ok(AnyOf2::A("Success".to_string()))
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct RequestUpload {
    built_by: String,
    completed_at: DateTime<Utc>,
    package: String,
    version: String,
    signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestUploadResponse {
    signed_url: String,
    object_path: String,
}

impl IntoResponse for RequestUploadResponse {
    fn into_response(self) -> Response {
        serde_json::to_string(&self).unwrap().into_response()
    }
}

pub async fn request_upload(
    State(state): State<AppState>,
    Json(q): Json<RequestUpload>,
)
    -> Result<AnyOf2<RequestUploadResponse, ApiError>, String>
{
    info!("api: handling request_upload {:?}", q);

    let mut conn = state.db.lock().await;
    let s3 = state.s3;

    match verify_maintainer(
        &mut *conn, "loap-upload-file", &q.built_by, &q.package, q.completed_at, &q.signature
    ).await? {
        AuthCheck::Ok(_, _) => (),
        err => return Ok(AnyOf2::B(ApiError::Auth(err))),
    }

    let path = format!("tarballs/{}_{}_{}.tar.xz", q.package, q.version, Uuid::new_v4());

    let url = s3.signed_url(
        Method::PUT,
        &object_store::path::Path::from(path.clone()),
        Duration::from_secs(7 * 60),
    )
        .await
        .map_err(|err| err.to_string())?;

    Ok(AnyOf2::A(RequestUploadResponse {
        signed_url: url.to_string(),
        object_path: path,
    }))
}

#[derive(Debug, Deserialize)]
pub struct ListPackages {
    m_id: Option<u32>,
    // TODO: for maintainer name
}

pub async fn list_packages(
    State(state): State<AppState>,
    Query(q): Query<ListPackages>,
)
    -> Result<AnyOf2<String, ApiError>, String>
{
    info!("api: handling list_packages {:?}", q);

    let packages = get_packages(&state, q.m_id)
        .await
        .map_err(|e| e.to_string())?;

    Ok(AnyOf2::A(
            serde_json::to_string(&packages).unwrap()
    ))
}

#[derive(Debug, Deserialize)]
pub struct ListBuilds {
    p: String,
    // TODO: by package id
}

pub async fn list_builds(
    State(state): State<AppState>,
    Query(q): Query<ListBuilds>,
)
    -> Result<AnyOf2<String, ApiError>, ServerError>
{
    info!("api: handling list_builds {:?}", q);

    let Some(pkg_id) = get_package_id(&state, &q.p).await? else {
        return Ok(AnyOf2::B(ApiError::NotFound));
    };

    let builds = get_builds_by_id(&state, pkg_id)
        .await?;

    Ok(AnyOf2::A(
            serde_json::to_string(&builds).unwrap()
    ))
}

