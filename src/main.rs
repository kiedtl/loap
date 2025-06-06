mod utils;

use std::net::Ipv4Addr;
use std::sync::Arc;
use tokio::sync::Mutex;

use hypertext::prelude::*;
use hypertext::Raw;

use anyhow::{Context, Result as AnyResult};
use axum::{extract::{Path, State}, routing::get, Router, http::StatusCode, response::IntoResponse};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use serde_json;
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
        .fallback_service(ServeDir::new("public"))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 3000)).await?;
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
    let builds = get_builds(&state, pkgname.clone()).await;

    maud! {
        Page title=(&pkgname) {
            h3 { (pkgname.clone()) }

            @match &builds {
                Ok(builds) if builds.is_empty() => {
                    p { "This package has never been built." }
                    p { "(If this package doesn't have a maintainer, maybe step in to help?)" }
                }
                Ok(builds) => table .list {
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
                Err(e) => p {
                    (format!("(Internal server error) couldn't fetch builds: {e}"))
                }
            }
        }
    }
}

async fn maintainer_page(
    State(state): State<AppState>,
    Path(maintainer_name): Path<String>
)
    -> impl IntoResponse
{
    let maintainer = get_maintainer(&state, maintainer_name.clone()).await;
    let packages =
        if let Ok(Some(maintainer)) = &maintainer {
            get_packages(&state, Some(maintainer.id))
                .await
                .unwrap_or(vec![])
        } else {
            vec![]
        };

    maud! {
        Page title=(&maintainer_name) {
            h3 { (maintainer_name.clone()) }
            @match &maintainer {
                Ok(Some(Maintainer { id, .. })) if *id == 0 || *id == 2 => div {
                    p {
                        @match *id {
                            USER_ORPH => "The orphanage is where packages go to die.",
                            USER_CEMT => "The cemetery is for dead packages.",
                            _ => p { "Unreachable" },
                        }
                    }
                    p {
                        @match *id {
                            USER_ORPH => "If you want to volunteer by uploading packages (and can do
                                          so semi-consistently), please contact kiedtl. Include an
                                          ED25519 SSH public key.",
                            USER_CEMT => "These packages don't meet the criteria for inclusion on LOAP.
                                          If you want these packages to be moved to the orphanage where someone
                                          can pick them up, please think of a plausible excuse first.",
                            _ => p { "Unreachable" },
                        }
                    }
                },
                Ok(Some(maintainer)) => div {
                    h4 { "Public Key" }
                    pre { (maintainer.pubkey.clone()) }
                    h4 { "Email" }
                    pre { (utils::fmt_email(&maintainer.email)) }
                    p { "(Don't copy-paste.)" }
                },
                Ok(None) => p { "This person doesn't exist." },
                Err(_) => p { "Error fetching maintainer." },
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
    }
}

async fn home_page(State(state): State<AppState>) -> impl IntoResponse {
    let packages = get_packages(&state, None).await;

    maud! {
        Page title="" {
            @if let Ok(packages) = &packages {
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
            } @else {
                p { "There was an error fetching packages. Please yell at kiedtl for this." }
            }
            (Raw(STATIC_ABOUT))
        }
    }
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

    let maintainer = sqlx::query_as::<_, Maintainer>(
        "SELECT id, name, email, pubkey from Maintainers WHERE name = $1;"
    )
        .bind(name)
        .fetch_one(&mut *conn)
        .await?;

    Ok(Some(maintainer))
}

async fn get_builds(state: &AppState, pkgname: String) -> AnyResult<Vec<Build>> {
    let mut conn = state.db.lock().await;

    let pkgid = sqlx::query("SELECT p.id FROM Packages p WHERE p.name = $1")
        .bind(pkgname)
        .fetch_one(&mut *conn)
        .await?
        .try_get::<i32, _>(0)?;

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
