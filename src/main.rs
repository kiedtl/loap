mod api;
mod utils;

use std::fmt;
use std::fs;
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

use log::{error, info};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

const USER_ORPH: u32 = 0;
const USER_CEMT: u32 = 2;

static FAVICON: &str = "R0lGODdhEAAQAKIDAAAAAP8AAP8AUP////9vb+SHhwAAAAAAACH5BAkAAAMALAAAAAAQABAAAANOOLrcC45BBcgE+NoBi61EiBXkiA1hKlYQRJAqvHGr92IineKfyqvAHShUyABsq1RxIBAEjjsdoOkMPHPSpxVwnXEw1vDz1ACHyREjWpEAADs=";
static STYLE: &str = include_str!("../assets/main.css");

#[derive(Debug, Clone)]
pub struct AppState {
    pub db: Arc<Mutex<SqliteConnection>>,
    pub s3: AmazonS3,
    pub pages: Vec<StaticPage>,
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
    package_id: u32,
    version: String,
    size: u32,
    completed_at: DateTime<Utc>,
    completed_in: u32,
    downloads: u32,
    object_path: String,
    builder_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
pub struct Package {
    id: u32,
    name: String,
    maintainer_id: u32,
    maintainer_name: String,
    #[sqlx(flatten)]
    repo: RepoInfo,
    build_version: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
pub struct RepoInfo {
    #[sqlx(rename = "repo_forge_url")]
    forge_url: String,
    #[sqlx(rename = "repo_name")]
    name: String,
    #[sqlx(rename = "repo_org")]
    org: String,
    #[sqlx(rename = "repo_dir")]
    dir: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
pub struct PackageInfo {
    id: u32,
    name: String,
    maintainer_name: String,
    #[sqlx(flatten)]
    repo: RepoInfo,
    build_count: u32,
    download_count: u32,
}

const NAV_PAGES: &[Page] = &[
    Page::Home, Page::About, Page::Stats, Page::Faq, Page::Orphanage, Page::Cemetery
];

#[derive(Clone, Debug, Deserialize)]
pub struct Frontmatter {
    page: Page,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StaticPage {
    meta: Frontmatter,
    name: String, // the filename w/o extension
    html: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub enum Page {
    Home,
    About,
    Stats,
    Faq,
    Orphanage,
    Cemetery,
    NotFound, // 404
    Error, // 500
    Other(String),
}

impl Page {
    pub fn title(&self) -> &str {
        match self {
            Page::Home => "packages",
            Page::About => "about",
            Page::Stats => "statistics",
            Page::Faq => "faq",
            Page::Orphanage => "orphanage",
            Page::Cemetery => "cemetery",
            Page::NotFound => "404",
            Page::Error => "500",
            Page::Other(s) => &s,
        }
    }

    pub fn href(&self) -> &'static str {
        match self {
            Page::Home => "/",
            Page::About => "/about",
            Page::Stats => "/ps",
            Page::Faq => "/faq",
            Page::Orphanage => "/m/orphanage",
            Page::Cemetery => "/m/cemetery",
            _ => unreachable!(),
        }
    }
}

macro_rules! try_or_500 {
    ($ex:expr) => {
        match $ex {
            Ok(value) => value,
            Err(err) => {
                error!("E: {:#}", err.to_string());
                return construct_500_page(err.into());
            },
        }
    }
}

#[tokio::main]
async fn main() -> AnyResult<()> {
    let port = std::env::var("PORT")
        .unwrap_or("3000".to_string())
        .parse::<u16>()
        .unwrap();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .or_else(|_| EnvFilter::try_new("loap=info,tower_http=trace"))
                .unwrap(),
        )
        .init();

    let mut pages = vec![];
    for entry in fs::read_dir("content")
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().unwrap().is_file())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "md"))
    {
        use gray_matter::Matter;
        use gray_matter::engine::YAML;
        use markdown;

        let matter = Matter::<YAML>::new();

        let path = entry.path();
        let raw = fs::read_to_string(&path).unwrap();
        let fname = path.file_stem().unwrap().to_string_lossy();

        let result = matter.parse_with_struct::<Frontmatter>(&raw).unwrap();
        let html = markdown::to_html(&result.content);

        pages.push(StaticPage {
            meta: result.data,
            name: fname.to_string(),
            html
        });
    }

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
        pages,
    };

    let app = Router::new()
        .route("/", get(home_page))
        .route("/ps", get(stats_page))
        .route("/{page}", get(static_page))
        .route("/p/{pkgname}", get(package_page))
        .route("/m/{maintainer_name}", get(maintainer_page))
        .route("/api/p/ls", get(api::list_packages))
        .route("/api/b/dl", get(api::download_by_build_id))
        .route("/api/b/upload", post(api::request_upload))
        .route("/api/b/report", post(api::report_build))
        .route("/api/b/ls", get(api::list_builds))
        .fallback_service(ServeDir::new("public"))
        .layer(TraceLayer::new_for_http())
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
    info!("page: package");

    let pkg_id = match get_package_id(&state, &pkgname).await {
        Err(e) => return construct_500_page(e),
        Ok(None) => return construct_404_page(),
        Ok(Some(pkg_id)) => pkg_id,
    };
    let package_info = try_or_500!(get_package_info(&state, pkg_id).await);
    let builds = try_or_500!(get_builds_by_id(&state, pkg_id).await);

    let total_size = builds.iter().fold(0, |a, b| a + b.size);
    let average_size = if builds.len() == 0 { 0 } else { total_size / builds.len() as u32 };

    maud! {
        Doc page=(Page::Other(pkgname.clone())) {
            h2 { "Package: " (pkgname.clone()) }

            div style="column-count: 2" {
                div {
                    b { "origin: " }
                    RepoView r=(&package_info.repo);
                }
                div { b { "maintainer: " } (package_info.maintainer_name) }
                div { b { "total builds: " } (package_info.build_count) }
                div { b { "total downloads: " } (package_info.download_count) }
                div { b { "average size: " } (utils::fmt_size(average_size)) }
                div { b { "total size: " } (utils::fmt_size(total_size)) }
            }

            h3 { "Builds" }

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
                            th { "dls" }
                            th { "link" }
                        }
                    }
                    tbody {
                        @for build in builds {
                            ({
                                let time_since = chrono::Utc::now()
                                    .signed_duration_since(build.completed_at)
                                    .num_seconds();
                                let dl = format!("/api/b/dl?id={}", build.id);

                                maud! {
                                    tr {
                                        td #m { (build.version) }
                                        td #m { (utils::fmt_size(build.size)) }
                                        td #m { (utils::fmt_duration(build.completed_in as _)) }
                                        td #m { (utils::fmt_duration(time_since))" ago" }
                                        td { (build.builder_name) }
                                        td #m { (build.downloads) }
                                        td { a .btn href=(dl) download { "Go" } }
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
    info!("page: maintainer");

    let maintainer = match get_maintainer(&state, maintainer_name.clone()).await {
        Ok(Some(m)) => m,
        Ok(None) => return maud! {
            Doc page=(Page::NotFound) {
                h2 { (maintainer_name.clone()) }
                p { "This person doesn't exist." }
                p { "(Want to change that? Ping someone in #kisslinux.)" }
            }
        }.render(),
        Err(e) => return construct_500_page(e),
    };

    let packages = try_or_500!(get_packages(&state, Some(maintainer.id)).await);

    let page = match maintainer.id {
        USER_ORPH => Page::Orphanage,
        USER_CEMT => Page::Cemetery,
        _ => Page::Other(maintainer_name.clone()),
    };

    maud! {
        Doc page=(page.clone()) {
            h2 { (maintainer_name.clone()) }

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
                        th { "info" }
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
    info!("page: home");

    let packages = try_or_500!(get_packages(&state, None).await);

    maud! {
        Doc page=(Page::Home) {
            h2 { "Packages" }

            p {
                "Use "
                    a href="https://github.com/kiedtl/loap/blob/trunk/tools/kiss-pig" { "kiss-pig" }
                " to download packages, and " code { "kiss i" } " as usual to install them."
            }

            table .list {
                thead {
                    tr {
                        th { "package" }
                        th { "origin" }
                        th { "version" }
                        th { "assigned" }
                        th { "info" }
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
            br;
            div style="text-align:center" {
                pre { r#"
                                       ____
                                      |    |
.------------------------------------.|    |
|                                     |    |
|   "The system is going down NOW!"   |    |
  |                                   ._|____|_.
'-----------------------------------\\|o_o |
                                      |:_/ |
                                     //   \ \
                                     (|    | )
                "# }
            }
        }
    }.render()
}

async fn stats_page(State(state): State<AppState>) -> impl IntoResponse {
    info!("page: stats");

    let mut conn = state.db.lock().await;

    #[derive(Clone, FromRow)]
    struct Item {
        pkg_name: String,
        downloads: u32,
        total_size: u32,
        avg_size: f32,
        avg_time: f32,
    }

    let items = try_or_500!(sqlx::query_as::<_, Item>(
        "SELECT
            p.name AS pkg_name,
            COALESCE(SUM(b.downloads), 0) AS downloads,
            COALESCE(SUM(b.size), 0) AS total_size,
            COALESCE(AVG(b.size), 0) AS avg_size,
            COALESCE(AVG(b.completed_in), 0) AS avg_time
        FROM Packages    p
        JOIN Builds      b ON b.package    = p.id
        JOIN Maintainers m ON p.maintainer = m.id
        WHERE m.id != 2 AND b.completed_in > 0 -- Exclude cemetery & never-built packages
        GROUP BY p.id
        ORDER BY downloads DESC, p.name ASC;"
    ).fetch_all(&mut *conn).await);

    let total_space_used = items.iter().fold(0, |a, b| a + b.total_size);
    let total_downloads = items.iter().fold(0, |a, b| a + b.downloads);

    let most_downloaded = items[0].clone();
    let least_downloaded = items[items.len() - 1].clone();

    macro_rules! wr {
        ($ex:expr) => {
            $ex.expect("should be at least one package in database").clone()
        }
    }

    let most_time = wr!(items.iter().max_by(|a, b| (a.avg_time as usize).cmp(&(b.avg_time as usize))));
    let least_time = wr!(items.iter().min_by(|a, b| (a.avg_time as usize).cmp(&(b.avg_time as usize))));
    let heaviest = wr!(items.iter().max_by(|a, b| (a.avg_size as usize).cmp(&(b.avg_size as usize))));

    maud! {
        Doc page=(Page::Stats) {
            h2 { "Statistics" }

            div style="column-count: 2;column-gap: 2.5em" {
                div {
                    b { "total size of tarballs: " }
                    (utils::fmt_size(total_space_used)) " / 10 GB"
                }
                div {
                    b { "largest package (avg): " }
                    (heaviest.pkg_name)
                    " (" (utils::fmt_size(heaviest.avg_size.round() as u32)) ")"
                }
                div {
                    b { "longest compiles (avg): " }
                    (most_time.pkg_name)
                    " (" (utils::fmt_duration(most_time.avg_time.round() as i64)) ")"
                }
                div {
                    b { "shortest compiles (avg): " }
                    (least_time.pkg_name)
                    " (" (utils::fmt_duration(least_time.avg_time.round() as i64)) ")"
                }
                div {
                    b { "total downloads: " }
                    (total_downloads)
                }
                div {
                    b { "most downloaded: " }
                    (most_downloaded.pkg_name) " (" (most_downloaded.downloads) ")"
                }
                div {
                    b { "least downloaded: " }
                    (least_downloaded.pkg_name) " (" (least_downloaded.downloads) ")"
                }
            }
        }
    }.render()
}

async fn static_page(
    State(state): State<AppState>,
    Path(path): Path<String>
) -> impl IntoResponse {
    for page in &state.pages {
        if page.name == path {
            info!("page: {}", page.name);
            return maud! {
                Doc page=(page.meta.page.clone()) {
                    (Raw(page.html.clone()))
                }
            }.render();
        }
    }

    info!("page: 404 for {}", path);
    construct_404_page()
}

fn construct_500_page(e: anyhow::Error) -> Rendered<String> {
    maud! {
        Doc page=(Page::Error) {
            h2 { "500 Internal Server Error" }
            p { (e.to_string()) }
        }
    }.render()
}

fn construct_404_page() -> Rendered<String> {
    maud! {
        Doc page=(Page::NotFound) {
            h2 { "404 Not Found" }
        }
    }.render()
}

struct Doc<R: Renderable> {
    page: Page,
    children: R,
}

impl<R: Renderable> Renderable for Doc<R> {
        fn render_to(&self, output: &mut String) {
            maud! {
                !DOCTYPE
                html {
                    head lang="en" {
                        meta charset="utf-8";
                        link href=(format!("data:image/gif;base64,{FAVICON}")) rel="icon";

                        script data-goatcounter="https://loap.goatcounter.com/count"
                            async src="//gc.zgo.at/count.js" { }

                        style { (Raw(STYLE)) }
                        title {
                            "LOAP — " (self.page.title())
                        }
                    }
                body {
                    nav {
                        h1 { a href="/" { "LIPSTICK\nON A PIG" } }
                        h3 { "Prebuilt packages for KISS Linux" }
                        hr;
                        br;
                        @for page in NAV_PAGES {
                            @if *page == self.page {
                                a .sel .nav href=(page.href()) { (page.title()) code { "*" } }
                            } @else {
                                a .nav href=(page.href()) { (page.title()) }
                            }
                        }
                    }
                    main {
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
    let mname = &p.maintainer_name;
    maud! {
        tr {
            td { (p.name.clone()) }
            td {
                RepoView r=(&p.repo);
            }
            td #m {
                (p.build_version.clone().unwrap_or("(none)".to_string()))
            }
            td { a href=(format!("/m/{}", mname)) { (mname.clone()) } }
            td { a .btn href=(format!("/p/{}", p.name)) { "View" } }
        }
    }
}

#[component]
fn repo_view<'a>(r: &'a RepoInfo) -> impl Renderable + use<'a> {
    maud! {
        a href=(r.forge_url) {
            (r.org)wbr;" → "(r.name)wbr;" → "(r.dir)
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

async fn get_package_info(
    state: &AppState,
    package_id: u32,
) -> AnyResult<PackageInfo> {
    let mut conn = state.db.lock().await;

    let info = sqlx::query_as::<_, PackageInfo>(
        "SELECT
            p.id, p.name,
            m.name      AS maintainer_name,
            r.name      AS repo_name,
            r.forge_url AS repo_forge_url,
            r.org       AS repo_org,
            r.dir       AS repo_dir,
            COUNT(b.id) AS build_count,
            COALESCE(SUM(b.downloads), 0) AS download_count
        FROM Packages p
        JOIN Maintainers  m ON m.id = p.maintainer
        JOIN Repositories r ON r.id = p.repository
        LEFT JOIN Builds  b ON p.id = b.package
        WHERE p.id = ?
        GROUP BY p.id;",
    )
        .bind(package_id)
        .fetch_one(&mut *conn)
        .await?;

    Ok(info)
}

async fn get_packages(
    state: &AppState,
    maintainer_id: Option<u32>,
) -> AnyResult<Vec<Package>> {
    let mut conn = state.db.lock().await;

    let query_text = format!(
        "SELECT
            p.id, p.name,
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
            ORDER BY completed_at DESC LIMIT 1
        )
        WHERE 1 {}
        ORDER BY p.name ASC;",
        if maintainer_id.is_some() { "AND m.id = ?" } else { "" },
    );

    let mut query = sqlx::query_as::<_, Package>(&query_text);

    if let Some(maintainer_id) = maintainer_id {
        query = query.bind(maintainer_id);
    }

    let mut packages = Vec::new();
    let mut rows = query.fetch(&mut *conn);

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

async fn get_builds_by_id(state: &AppState, pkgid: u32) -> AnyResult<Vec<Build>> {
    let mut conn = state.db.lock().await;

    let mut builds = Vec::new();
    let mut rows = sqlx::query_as::<_, Build>(
        "SELECT
            b.id, b.version, b.size, b.completed_at, b.completed_in,
            b.downloads, b.object_path,
            b.package as package_id,
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
