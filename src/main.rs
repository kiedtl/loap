mod api;
mod utils;

use std::fmt;
use std::net::Ipv4Addr;
use std::sync::Arc;
use tokio::sync::Mutex;

use hypertext::prelude::*;
use hypertext::Raw;

use anyhow::{Context, Result as AnyResult};
use axum::{
    extract::{Path, State},
    routing::{post, get}, Router,
    response::{IntoResponse},
};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use serde::{Serialize, Deserialize};
use serde_repr::{Serialize_repr, Deserialize_repr};
use sqlx::{Row, FromRow, Connection, SqliteConnection};
use tower_http::services::ServeDir;

const USER_ORPH: u32 = 0;
const USER_CEMT: u32 = 2;

static FAVICON: &str = "R0lGODdhEAAQAKIDAAAAAP8AAP8AUP////9vb+SHhwAAAAAAACH5BAkAAAMALAAAAAAQABAAAANOOLrcC45BBcgE+NoBi61EiBXkiA1hKlYQRJAqvHGr92IineKfyqvAHShUyABsq1RxIBAEjjsdoOkMPHPSpxVwnXEw1vDz1ACHyREjWpEAADs=";

static STATIC_STYLE: &str = include_str!("../static/main.css");
static STATIC_ABOUT: &str = include_str!("../static/build/about.html");

#[derive(Debug, Clone)]
pub struct AppState {
    pub db: Arc<Mutex<SqliteConnection>>,
    pub s3: AmazonS3,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize_repr, Deserialize_repr)]
#[repr(u16)]
pub enum BuildResult {
    Success = 0,
    Failure = 1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct Maintainer {
    id: u32,
    name: String,
    email: String,
    pubkey: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct Build {
    id: u32,
    version: String,
    size: u32,
    completed_at: DateTime<Utc>,
    completed_in: u32,
    downloads: u32,
    object_path: String,
    builder_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct Package {
    name: String,
    maintainer_id: u32,
    maintainer_name: String,
    repo_forge_url: String,
    repo_name: String,
    repo_org: String,
    repo_dir: String,
    build_version: Option<String>,
}


#[tokio::main]
async fn main() -> AnyResult<()> {
    let port = std::env::var("PORT")
        .unwrap_or("3000".to_string())
        .parse::<u16>()
        .unwrap();

    let state = AppState {
        db: Arc::new(Mutex::new(
                SqliteConnection::connect("sqlite:main.sqlite3")
                    .await
                    .with_context(|| "Couldn't open database")?,
        )),
        s3: AmazonS3Builder::from_env()
            .with_endpoint("https://s3.us-east-005.backblazeb2.com")
            .with_region("s3.us-east-005")
            .with_bucket_name("kisslinux")
            .build()
            .with_context(|| "Couldn't create S3 state")?,
    };

    let app = Router::new()
        .route("/", get(home_page))
        .route("/p/{pkgname}", get(package_page))
        .route("/m/{maintainer_name}", get(maintainer_page))
        .route("/api/p/ls", get(api::list_packages))
        .route("/api/b/dl", get(api::download_by_build_id))
        .route("/api/b/upload", post(api::request_upload))
        .route("/api/b/report", post(api::report_build))
        .route("/api/b/ls", get(api::list_builds))
        .fallback_service(ServeDir::new("public"))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await?;
    axum::serve(listener, app.into_make_service())
        .await?;

    Ok(())
}

async fn package_page(
    State(state): State<AppState>,
    Path(pkgname): Path<String>
)
    -> impl IntoResponse
{
    let pkg_id = match get_package_id(&state, &pkgname).await {
        Err(e) => return construct_500_page(e),
        Ok(None) => return construct_404_page(),
        Ok(Some(pkg_id)) => pkg_id,
    };

    let builds = match get_builds(&state, pkg_id).await {
        Ok(b) => b,
        Err(e) => return construct_500_page(e),
    };

    maud! {
        Page title=(&pkgname) {
            h3 { (pkgname.clone()) }

            @match &builds {
                builds if builds.is_empty() => {
                    p { "This package has never been built." }
                    p { "(If this package doesn't have a maintainer, maybe step in to help?)" }
                }
                builds => table .list {
                    thead {
                        tr {
                            th { "version" }
                            th { "size" }
                            th { "time" }
                            th { "completed" }
                            th { "builder" }
                            th { "downloads" }
                            th { "link" }
                        }
                    }
                    tbody {
                        @for build in builds {
                            ({
                                let time_since = chrono::Utc::now()
                                    .signed_duration_since(build.completed_at)
                                    .num_seconds();
                                let dl = format!("/api/dl?id={}", build.id);

                                maud! {
                                    tr {
                                        td #m { (build.version) }
                                        td #m { (utils::fmt_size(build.size)) }
                                        td #m { (utils::fmt_duration(build.completed_in as _)) }
                                        td #m { (utils::fmt_duration(time_since))" ago" }
                                        td { (build.builder_name) }
                                        td #m { (build.downloads) }
                                        td { a .btn href=(dl) download { "Download" } }
                                    }
                                }
                            })
                        }
                    }
                }
            }
        }
    }.render()
}

async fn maintainer_page(
    State(state): State<AppState>,
    Path(maintainer_name): Path<String>
)
    -> impl IntoResponse
{
    let maintainer = match get_maintainer(&state, maintainer_name.clone()).await {
        Ok(Some(m)) => m,
        Ok(None) => return maud! {
            Page title="404" {
                h3 { (maintainer_name.clone()) }
                p { "This person doesn't exist." }
                p { "(Want to change that? Ping someone in #kisslinux.)" }
            }
        }.render(),
        Err(e) => return construct_500_page(e),
    };

    let packages = match get_packages(&state, Some(maintainer.id)).await {
        Ok(p) => p,
        Err(e) => return construct_500_page(e),
    };

    maud! {
        Page title=(&maintainer_name) {
            h3 { (maintainer_name.clone()) }

            @if maintainer.id == USER_ORPH {
                p {
                    "The orphanage is where packages go to die."
                }
                p {
                    "If you want to volunteer by uploading packages (and can do
                    so semi-consistently), please contact kiedtl. Include an
                    ED25519 SSH public key."
                }
            } @else if maintainer.id == USER_CEMT {
                p {
                    "The cemetery is for dead packages."
                }
                p {
                    "These packages don't meet the criteria for inclusion on LOAP.
                    If you want these packages to be moved to the orphanage where someone
                    can pick them up, please think of a plausible excuse first."
                }
            } @else {
                h4 { "Public Key" }
                pre { (maintainer.pubkey.clone()) }
                h4 { "Email" }
                pre { (utils::fmt_email(&maintainer.email)) }
                p { "(Don't copy-paste.)" }
            }

            h4 { "Packages" }
            table .list {
                thead {
                    tr {
                        th { "package" }
                        th { "origin" }
                        th { "version" }
                        th { "assigned" }
                        th { "builds" }
                    }
                }
                tbody {
                    @for pkg in packages.iter() {
                        PackageView p=pkg;
                    }
                }
            }
        }
    }.render()
}

async fn home_page(State(state): State<AppState>) -> impl IntoResponse {
    let packages = match get_packages(&state, None).await {
        Ok(p) => p,
        Err(e) => return construct_500_page(e),
    };

    maud! {
        Page title="" {
            table .list {
                thead {
                    tr {
                        th { "package" }
                        th { "origin" }
                        th { "version" }
                        th { "assigned" }
                        th { "builds" }
                    }
                }
                tbody {
                    @for pkg in packages.iter() {
                        @if pkg.maintainer_id != USER_CEMT {
                            PackageView p=pkg;
                        }
                    }
                }
            }

            (Raw(STATIC_ABOUT))
        }
    }.render()
}

fn construct_500_page(e: anyhow::Error) -> Rendered<String> {
    maud! {
        Page title="500" {
            h3 { "500 Internal Server Error" }
            p { (e.to_string()) }
        }
    }.render()
}

fn construct_404_page() -> Rendered<String> {
    maud! {
        Page title="404" {
            h3 { "404 Not Found" }
        }
    }.render()
}

struct Page<'a, R: Renderable> {
    title: &'a str,
    children: R,
}

impl<R: Renderable> Renderable for Page<'_, R> {
    fn render_to(&self, output: &mut String) {
        maud! {
            !DOCTYPE
            html {
                head lang="en" {
                    meta charset="utf-8";
                    link href=(format!("data:image/gif;base64,{FAVICON}")) rel="icon";

                    script data-goatcounter="https://MYCODE.goatcounter.com/count"
                        async src="//gc.zgo.at/count.js" { }

                    style { (Raw(STATIC_STYLE)) }
                    title {
                        @if self.title.is_empty() {
                            "LOAP"
                        } @else {
                            "LOAP —" (self.title.to_owned())
                        }
                    }
                }
                body {
                    main {
                        table {
                            tbody {
                                tr {
                                    td { h1 { a href="/" { "Lipstick on a Pig" } } }
                                    td style="text-align:right" { h2 { "Prebuilt packages for KISS Linux" } }
                                }
                            }
                        }
                        hr;
                        br;
                        (self.children)
                    }
                }
            }
        }
        .render_to(output);
    }
}

#[component]
fn package_view<'a>(p: &'a Package) -> impl Renderable + use<'a> {
    let package = p;
    let mname = &package.maintainer_name;
    maud! {
        tr {
            td { (package.name.clone()) }
            td {
                a href=(package.repo_forge_url) {
                    (package.repo_org.clone())" → "(package.repo_name.clone())" → "(package.repo_dir.clone())
                }
            }
            td #m {
                (package.build_version.clone().unwrap_or("(none)".to_string()))
            }
            td { a href=(format!("/m/{}", mname)) { (mname.clone()) } }
            td { a .btn href=(format!("/p/{}", package.name)) { "View" } }
        }
    }
}

#[derive(Copy, Clone, Debug)]
enum AuthCheck {
    Ok(i32, i32), // maintainer_id, package_id
    SignatureMalformed, // Signature is invalid base64
    SignatureFailed, // Signature check failed
    NotKnown, // Either package or maintainer doesn't exist
    NotMaintainer, // Not correct maintainer
}

impl fmt::Display for AuthCheck {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            AuthCheck::Ok(_, _) => write!(f, "Success"),
            AuthCheck::SignatureMalformed => write!(f, "Malformed signature"),
            AuthCheck::SignatureFailed => write!(f, "Signature verification failed"),
            AuthCheck::NotKnown => write!(f, "Unknown maintainer or package"),
            AuthCheck::NotMaintainer => write!(f, "Not correct maintainer"),
        }
    }
}

async fn get_packages(
    state: &AppState,
    maintainer_id: Option<u32>
) -> AnyResult<Vec<Package>> {
    let mut conn = state.db.lock().await;

    let query_text = format!(
        "SELECT
            p.name,
            m.id        AS maintainer_id,
            m.name      AS maintainer_name,
            r.name      AS repo_name,
            r.forge_url AS repo_forge_url,
            r.org       AS repo_org,
            r.dir       AS repo_dir,
            b.version   AS build_version
        FROM Packages p
        JOIN Maintainers  m ON m.id = p.maintainer
        JOIN Repositories r ON r.id = p.repository
        LEFT JOIN Builds  b ON b.id = (
            SELECT id FROM Builds
            WHERE package = p.id
            ORDER BY completed_at LIMIT 1
        )
        {}
        ORDER BY p.name ASC;",
        if maintainer_id.is_some() {
            "WHERE m.id = $1"
        } else {
            ""
        }
    );

    let mut packages = Vec::new();
    let mut rows = if let Some(maintainer_id) = maintainer_id {
        sqlx::query_as::<_, Package>(&query_text)
            .bind(maintainer_id)
    } else {
        sqlx::query_as::<_, Package>(&query_text)
    }
        .fetch(&mut *conn);
    while let Some(row) = rows.try_next().await? {
        packages.push(row);
    }

    Ok(packages)
}

async fn get_maintainer(state: &AppState, name: String) -> AnyResult<Option<Maintainer>> {
    let mut conn = state.db.lock().await;

    let query = sqlx::query_as::<_, Maintainer>(
        "SELECT id, name, email, pubkey from Maintainers WHERE name = $1;"
    )
        .bind(name)
        .fetch_one(&mut *conn)
        .await;

    Ok(match query {
        Ok(maintainer) => Some(maintainer),
        Err(sqlx::Error::RowNotFound) => None,
        Err(e) => return Err(e.into()),
    })
}

async fn get_package_id(state: &AppState, pkgname: &str) -> AnyResult<Option<u32>> {
    let mut conn = state.db.lock().await;

    let q = sqlx::query("SELECT p.id FROM Packages p WHERE p.name = $1")
        .bind(pkgname)
        .fetch_one(&mut *conn)
        .await;

    Ok(match q {
        Ok(row) => Some(row.try_get::<u32, _>(0)?),
        Err(sqlx::Error::RowNotFound) => None,
        Err(e) => return Err(e.into()),
    })
}

async fn get_builds(state: &AppState, pkgid: u32) -> AnyResult<Vec<Build>> {
    let mut conn = state.db.lock().await;

    let mut builds = Vec::new();
    let mut rows = sqlx::query_as::<_, Build>(
        "SELECT
            b.id, b.version, b.size, b.completed_at, b.completed_in,
            b.downloads, b.object_path,
            m.name as builder_name
        FROM Builds b
        JOIN Maintainers m ON m.id = b.built_by
        WHERE b.package = $1
        ORDER BY b.completed_at DESC;"
    )
        .bind(pkgid)
        .fetch(&mut *conn);
    while let Some(row) = rows.try_next().await? {
        builds.push(row);
    }

    Ok(builds)
}

async fn verify_maintainer(
    conn: &mut sqlx::SqliteConnection,
    namespace: &str,
    maintainer: &str,
    package: &str,
    timestamp: DateTime<Utc>,
    signature: &str,
)
    -> Result<AuthCheck, String>
{
    use ssh_key::{PublicKey, SshSig};

    let data = sqlx::query(
        "SELECT
            m.id, m.pubkey,
            p.id, p.maintainer
        FROM Maintainers m
        INNER JOIN Packages p ON p.name = $2 AND p.maintainer = m.id
        WHERE m.name = $1;"
    )
        .bind(&maintainer)
        .bind(&package)
        .fetch_one(&mut *conn)
        .await;

    let data = match data {
        Ok(d) => d,
        Err(sqlx::Error::RowNotFound) => return Ok(AuthCheck::NotKnown),
        Err(e) => return Err(e.to_string()),
    };

    let maintainer_id = data.try_get::<i32, _>(0).map_err(|e| e.to_string())?;
    let pubkey = data.try_get::<String, _>(1).map_err(|e| e.to_string())?;
    let pkg_id = data.try_get::<i32, _>(2).map_err(|e| e.to_string())?;
    let pkg_maintainer = data.try_get::<i32, _>(3).map_err(|e| e.to_string())?;

    if maintainer_id != pkg_maintainer {
        return Ok(AuthCheck::NotMaintainer);
    }

    let key = pubkey.parse::<PublicKey>()
        .map_err(|e| format!("Couldn't parse public key: {e}"))?;

    let Ok(parsed_signature) = signature.parse::<SshSig>() else {
        return Ok(AuthCheck::SignatureMalformed);
    };

    let message = format!("{maintainer}:{package}:{}", timestamp.timestamp());

    Ok(match key.verify(namespace, message.as_bytes(), &parsed_signature) {
        Ok(_) => AuthCheck::Ok(maintainer_id, pkg_id),
        Err(_) => AuthCheck::SignatureFailed,
    })
}
