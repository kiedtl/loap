use crate::utils;

use chrono::{DateTime, Utc};

use leptos::either::{Either, EitherOf3};
use leptos::server_fn::codec::{Json, GetUrl};
use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::{
    components::{Route, Router, Routes},
    hooks::use_params_map,
    StaticSegment,
};
use leptos_router::path;
use serde::{Serialize, Deserialize};
use serde_repr::{Serialize_repr, Deserialize_repr};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize_repr, Deserialize_repr)]
#[repr(u16)]
pub enum BuildResult {
    Success = 0,
    Failure = 1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ssr", derive(sqlx::FromRow))]
pub struct Maintainer {
    id: u32,
    name: String,
    email: String,
    pubkey: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ssr", derive(sqlx::FromRow))]
pub struct Package {
    name: String,
    maintainer_name: String,
    repo_forge_url: String,
    repo_name: String,
    repo_org: String,
    repo_dir: String,
    build_version: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ssr", derive(sqlx::FromRow))]
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

#[cfg(feature = "ssr")]
pub mod ssr {
    pub use futures::TryStreamExt;
    pub use http::{StatusCode, method::Method};
    pub use leptos::server_fn::ServerFnError;
    pub use leptos_axum::{self, ResponseOptions};
    pub use object_store::aws::{AmazonS3, AmazonS3Builder};
    pub use object_store::ObjectStore;
    pub use object_store::path::Path;
    pub use object_store::signer::Signer;
    pub use sqlx::{Connection, SqliteConnection, Row};
    pub use std::time::Duration;
    pub use uuid::Uuid;

    use axum::extract::FromRef;
    use leptos::prelude::LeptosOptions;

    use std::sync::Arc;
    use tokio::sync::{Mutex, OwnedMutexGuard};

    #[derive(FromRef, Debug, Clone)]
    pub struct AppState {
        pub leptos_options: LeptosOptions,
        pub db: Arc<Mutex<SqliteConnection>>,
        pub s3: AmazonS3,
    }

    pub fn state() -> Result<AppState, ServerFnError> {
        leptos::context::use_context::<AppState>()
            .ok_or_else(|| ServerFnError::new("Application state missing."))
    }

    pub async fn db() -> Result<OwnedMutexGuard<SqliteConnection>, ServerFnError> {
        Ok(state()?.db.clone().lock_owned().await)
    }

    pub async fn s3() -> Result<AmazonS3, ServerFnError> {
        Ok(state()?.s3)
    }
}

#[server(endpoint = "dl", input = GetUrl)]
pub async fn api_download_by_build_id(id: u32) -> Result<String, ServerFnError> {
    use self::ssr::*;

    let mut conn = db().await?;
    let s3 = s3().await?;

    let query_result = sqlx::query("SELECT object_path FROM Builds where id = $1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await;

    let path = match query_result {
        Ok(d) => d.try_get::<String, _>(0)?,
        Err(sqlx::Error::RowNotFound) => {
            let response = expect_context::<ResponseOptions>();
            response.set_status(StatusCode::NOT_FOUND);
            return Ok("Build not found.".to_string());
        },
        Err(e) => return Err(e.into()),
    };

    let url = s3.signed_url(Method::GET, &Path::from(path.clone()), Duration::from_secs(3 * 60))
        .await?;

    sqlx::query("UPDATE Builds SET downloads = downloads + 1 WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;

    leptos_axum::redirect(url.as_str());
    Ok("Redirecting...".to_string())
}

#[cfg(feature = "ssr")]
#[derive(Copy, Clone, Debug)]
enum AuthCheck {
    Ok(i32, i32), // maintainer_id, package_id
    SignatureMalformed, // Signature is invalid base64
    SignatureFailed, // Signature check failed
    NotKnown, // Either package or maintainer doesn't exist
    NotMaintainer, // Not correct maintainer
}

#[cfg(feature = "ssr")]
async fn verify_maintainer(
    conn: &mut sqlx::SqliteConnection,
    namespace: &str,
    maintainer: &str,
    package: &str,
    timestamp: DateTime<Utc>,
    signature: &str,
)
    -> Result<AuthCheck, ServerFnError>
{
    use ssh_key::{PublicKey, SshSig};
    use self::ssr::*;

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
        Err(e) => return Err(e.into()),
    };

    let maintainer_id = data.try_get::<i32, _>(0)?;
    let pubkey = data.try_get::<String, _>(1)?;
    let pkg_id = data.try_get::<i32, _>(2)?;
    let pkg_maintainer = data.try_get::<i32, _>(3)?;

    if maintainer_id != pkg_maintainer {
        return Ok(AuthCheck::NotMaintainer);
    }

    let key = pubkey.parse::<PublicKey>()
        .map_err(|e| ServerFnError::new(format!("Couldn't parse public key: {e}")))?;

    let Ok(parsed_signature) = signature.parse::<SshSig>() else {
        return Ok(AuthCheck::SignatureMalformed);
    };

    let message = format!("{maintainer}:{package}:{}", timestamp.timestamp());

    Ok(match key.verify(namespace, message.as_bytes(), &parsed_signature) {
        Ok(_) => AuthCheck::Ok(maintainer_id, pkg_id),
        Err(_) => AuthCheck::SignatureFailed,
    })
}

#[server(endpoint = "report", input = Json)]
pub async fn api_report_build(
    built_by: String,
    package: String,
    version: String,
    completed_in: u32,
    completed_at: DateTime<Utc>,
    object_path: String,
    signature: String,
)
    -> Result<String, ServerFnError>
{
    use self::ssr::*;

    let mut conn = db().await?;
    let s3 = s3().await?;

    let (maintainer_id, pkg_id) = match verify_maintainer(
        &mut *conn, "loap-upload-file", &built_by, &package, completed_at, &signature
    ).await? {
        AuthCheck::Ok(mi, pi) => (mi, pi),
        err => {
            let response = expect_context::<ResponseOptions>();
            response.set_status(StatusCode::FORBIDDEN);
            return Ok(format!("{:?}", err));
        },
    };

    let res = s3.get_opts(
        &Path::from(object_path.clone()),
        object_store::GetOptions { head: true, .. Default::default() },
    ).await;

    let size = match res {
        // FIXME avoid cast somehow? (sqlite doesn't have u64 though...)
        Ok(object_store::GetResult { meta, .. }) => meta.size as u32,
        Err(_) => Err(ServerFnError::new("Object inaccessible or not found."))?,
    };

    let r = sqlx::query(
        "INSERT INTO Builds
            (built_by, package, version, size, completed_at, completed_in, result, downloads, object_path)
        VALUES
            ($1, $2, $3, $4, $5, $6, 0, 0, $7);"
    )
        .bind(maintainer_id)
        .bind(pkg_id)
        .bind(version)
        .bind(size)
        .bind(completed_at.timestamp())
        .bind(completed_in)
        .bind(object_path)
        .execute(&mut *conn).await?;

    if r.rows_affected() != 1 {
        Err(ServerFnError::new("Internal database error!"))
    } else {
        Ok("Success".to_string())
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct UploadUrlResponse {
    signed_url: String,
    object_path: String,
}

#[server(endpoint = "upload", input = Json)]
pub async fn api_upload_url(
    built_by: String,
    completed_at: DateTime<Utc>,
    package: String,
    version: String,
    signature: String,
)
    -> Result<Result<UploadUrlResponse, String>, ServerFnError>
{
    use self::ssr::*;
    use std::time::Duration;

    let mut conn = db().await?;

    match verify_maintainer(
        &mut *conn, "loap-upload-file", &built_by, &package, completed_at, &signature
    ).await? {
        AuthCheck::Ok(_, _) => (),
        err => {
            let response = expect_context::<ResponseOptions>();
            response.set_status(StatusCode::FORBIDDEN);
            return Ok(Err(format!("{:?}", err)));
        },
    }

    let s3 = AmazonS3Builder::from_env()
        .with_endpoint("https://s3.us-east-005.backblazeb2.com")
        .with_region("s3.us-east-005")
        .with_bucket_name("kisslinux")
        .build()?;

    let path = format!("tarballs/{package}_{version}_{}.tar.xz", Uuid::new_v4());

    let url = s3.signed_url(
        Method::PUT,
        &Path::from(path.clone()),
        Duration::from_secs(7 * 60),
    )
        .await?;

    Ok(Ok(UploadUrlResponse {
        signed_url: url.to_string(),
        object_path: path,
    }))
}

#[server(endpoint = "lsmaintainers", input = GetUrl)]
pub async fn get_maintainer(name: String) -> Result<Option<Maintainer>, ServerFnError> {
    use self::ssr::*;
    let mut conn = db().await?;

    let query_result = sqlx::query_as::<_, Maintainer>(
        "SELECT id, name, email, pubkey from Maintainers WHERE name = $1;"
    )
        .bind(name)
        .fetch_one(&mut *conn)
        .await;

    let maintainer = match query_result {
        Ok(m) => m,
        Err(sqlx::Error::RowNotFound) => {
            let response = expect_context::<ResponseOptions>();
            response.set_status(StatusCode::NOT_FOUND);
            return Ok(None);
        },
        Err(e) => return Err(e.into()),
    };

    Ok(Some(maintainer))
}

#[server(endpoint = "lspkgs", input = GetUrl)]
pub async fn get_packages(maintainer_id: Option<u32>) -> Result<Vec<Package>, ServerFnError> {
    use self::ssr::*;

    let mut conn = db().await?;

    let query_text = format!(
        "SELECT
            p.name,
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
        ) {}",
        if maintainer_id.is_some() {
            "WHERE m.id = $1;"
        } else {
            ";"
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

// TODO: Allow getting packages by id. Would make it better for links directly
// from the website, since there'd be one less query.
#[server]
pub async fn get_builds(pkgname: String) -> Result<Vec<Build>, ServerFnError> {
    use self::ssr::*;

    let mut conn = db().await?;

    let query_result = sqlx::query("SELECT p.id FROM Packages p WHERE p.name = $1")
        .bind(pkgname)
        .fetch_one(&mut *conn)
        .await;

    let pkgid = match query_result {
        Ok(d) => d.try_get::<i32, _>(0)?,
        Err(sqlx::Error::RowNotFound) => return Err(ServerFnError::new("No such package.")),
        Err(e) => return Err(e.into()),
    };

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

pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <AutoReload options=options.clone() />
                <HydrationScripts options/>
                <MetaTags/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    // Provides context that manages stylesheets, titles, meta tags, etc.
    provide_meta_context();

    view! {
        <Stylesheet id="leptos" href="/pkg/loap.css"/>
        <Title text="LOAP Packages"/>

        <Router>
            <main>
                <table>
                    <tr>
                        <td><h1>Lipstick On a Pig</h1></td>
                        <td style="text-align:right"><h2>Prebuilt packages for KISS Linux</h2></td>
                    </tr>
                </table>
                <hr />
                <Routes fallback=|| "Page not found.".into_view()>
                    <Route path=StaticSegment("") view=HomePage/>
                    <Route path=path!("/p/:pkgname") view=Package/>
                    <Route path=path!("/m/:maintainer") view=Maintainer/>
                </Routes>
            </main>
        </Router>
    }
}

#[component]
fn PackageEntry(package: Package) -> impl IntoView {
    let mname = &package.maintainer_name;
    view! {
        <tr>
            <td>{package.name.clone()}</td>
            <td>
                <a href={package.repo_forge_url.clone()}>
                    {package.repo_org.clone()}" → "{package.repo_name.clone()}" → "{package.repo_dir.clone()}
                </a>
            </td>
            <td id="m">{package.build_version.clone().unwrap_or("(none)".to_string())}</td>
            <td><a href={format!("/m/{}", mname)}>{mname.clone()}</a></td>
            <td><a href={format!("/p/{}", package.name)} class="btn">"View"</a></td>
        </tr>
    }
}

#[component]
fn HomePage() -> impl IntoView {
    let packages = OnceResource::new(get_packages(None));

    let existing_packages = move || Suspend::new(async move {
        packages.await.map(|packages| {
            if packages.is_empty() {
                return Either::Left(view! { <p>"No packages were found."</p> });
            }

            Either::Right(
                packages.iter()
                    .map(move |package| view! { <PackageEntry package=package.clone() /> })
                    .collect::<Vec<_>>(),
            )
        })
    });

    view! {
        <div>
            <table class="list">
                <thead>
                    <tr>
                        <th>package</th>
                        <th>origin</th>
                        <th>version</th>
                        <th>assigned</th>
                        <th>builds</th>
                    </tr>
                </thead>
                <tbody>
                    <Transition fallback=move || view! { <tr><td>"Loading..."</td></tr> }>
                        {existing_packages}
                    </Transition>
                </tbody>
            </table>
        </div>
    }
}

#[component]
fn Maintainer() -> impl IntoView {
    let params = use_params_map();
    let param_maintainer = move || params.read().get("maintainer");

    let maintainer_name = match param_maintainer() {
        Some(maintainer) => maintainer,
        // NOTE: this is actually unreachable, since null /m/ routes aren't routed here anyway.
        None => return view!{ <p>"You want to see a maintainer or no?"</p> }.into_any(),
    };

    let maintainer = OnceResource::new(get_maintainer(maintainer_name.clone()));
    let packages = Resource::new(
        move || maintainer.track(),
        move |_| async move {
            if let Ok(Some(maintainer)) = maintainer.await {
                get_packages(Some(maintainer.id)).await
            } else {
                Ok(vec![])
            }
        },
    );

    let existing_packages = move || Suspend::new(async move {
        packages.await.map(|packages| {
            if packages.is_empty() {
                return Either::Left(view! { <p>"This maintainer has no packages."</p> });
            }

            Either::Right(
                packages.iter()
                    .map(move |package| view! { <PackageEntry package=package.clone() /> })
                    .collect::<Vec<_>>(),
            )
        })
    });

    let maintainer_view = move || Suspend::new(async move {
        maintainer.await.map(|maintainer| match maintainer {
            Some(Maintainer { id, .. }) if id == 0 => EitherOf3::A(view! {
                <p>
                    "The orphanage is where packages go to die."
                </p>
                <p>
                    "If you want to volunteer by uploading packages (and can do
                    so semi-consistently), please contact kiedtl. Include an
                    ED25519 SSH public key."
                </p>
            }),
            Some(maintainer) => EitherOf3::B(view! {
                <h4>Public Key</h4>
                <pre>{maintainer.pubkey.clone()}</pre>
                <h4>Email</h4>
                <pre>{utils::fmt_email(&maintainer.email)}</pre>
                <p>"(Don't copy-paste.)"</p>
            }),
            None => EitherOf3::C(view!{}),
        })
    });

    view! {
        <div>
            <h3>{maintainer_name.clone()}</h3>
            {maintainer_view}
            <h4>Packages</h4>
            <table class="list">
                <thead>
                    <tr>
                        <th>package</th>
                        <th>origin</th>
                        <th>version</th>
                        <th>assigned</th>
                        <th>builds</th>
                    </tr>
                </thead>
                <tbody>
                    <Transition fallback=move || view! { <tr><td>"Loading..."</td></tr> }>
                        {existing_packages}
                    </Transition>
                </tbody>
            </table>
        </div>
    }.into_any()
}

#[component]
fn Package() -> impl IntoView {
    let params = use_params_map();
    let param_pkgname = move || params.read().get("pkgname");

    let pkgname = match param_pkgname() {
        Some(pkgname) => pkgname,
        // NOTE: this is actually unreachable, since null /p/ routes aren't routed here anyway.
        None => return view!{ <p>"You want to see a package or no?"</p> }.into_any(),
    };

    let builds = OnceResource::new(get_builds(pkgname.clone()));

    let existing_builds = move || Suspend::new(async move {
        builds.await.map(|builds| {
            if builds.is_empty() {
                return Either::Left(view! {
                    <p>"This package has never been built."</p>
                    <p>"(If this package doesn't have a maintainer, maybe step in to help?)"</p>
                });
            }

            Either::Right(
                builds.iter().map(move |build| {
                    let time_since = chrono::Utc::now()
                        .signed_duration_since(build.completed_at)
                        .num_seconds();
                    let dl = format!("/api/dl?id={}", build.id);
                    view! {
                        <tr>
                            <td id="m">{build.version.clone()}</td>
                            <td id="m">{utils::fmt_size(build.size)}</td>
                            <td id="m">{utils::fmt_duration(build.completed_in as _)}</td>
                            <td id="m">{utils::fmt_duration(time_since)}" ago"</td>
                            <td>{build.builder_name.clone()}</td>
                            <td id="m">{build.downloads}</td>
                            <td><a href={dl} class="btn" download>"Download"</a></td>
                        </tr>
                    }
                })
                .collect::<Vec<_>>(),
            )
        })
    });

    view! {
        <h3>{pkgname.clone()}</h3>
        <div>
            <table class="list">
                <thead>
                    <tr>
                        <th>version</th>
                        <th>size</th>
                        <th>time</th>
                        <th>completed</th>
                        <th>builder</th>
                        <th>downloads</th>
                        <th>link</th>
                    </tr>
                </thead>
                <tbody>
                    <Transition fallback=move || view! { <tr><td>"Loading..."</td></tr> }>
                        {existing_builds}
                    </Transition>
                </tbody>
            </table>
        </div>
    }.into_any()
}
