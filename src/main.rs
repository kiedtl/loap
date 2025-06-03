#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use std::sync::Arc;
    use tokio::sync::Mutex;

    use axum::{
        body::Body as AxumBody,
        extract::{Path, State},
        http::Request,
        response::IntoResponse,
        routing::get,
        Router,
    };
    use leptos_axum::handle_server_fns_with_context;
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use object_store::aws::AmazonS3Builder;
    use sqlx::{Connection, SqliteConnection};

    use loap::app::*;
    use loap::app::ssr::AppState;

    async fn server_fn_handler(
        State(app_state): State<AppState>,
        _path: Path<String>,
        request: Request<AxumBody>,
    ) -> impl IntoResponse {
        handle_server_fns_with_context(
            move || {
                provide_context(app_state.clone());
            },
            request,
        )
        .await
    }

    pub async fn leptos_routes_handler(
        State(app_state): State<AppState>,
        State(_option): State<leptos::prelude::LeptosOptions>,
        request: Request<AxumBody>,
    ) -> axum::response::Response {
        let lo = app_state.leptos_options.clone();
        let handler = leptos_axum::render_app_async_with_context(
            move || {
                provide_context(app_state.clone());
            },
            move || shell(lo.clone()),
        );

        handler(request).await.into_response()
    }

    let conf = get_configuration(None).unwrap();
    let addr = conf.leptos_options.site_addr;

    let state = AppState {
        leptos_options: conf.leptos_options,
        db: Arc::new(Mutex::new(
                SqliteConnection::connect("sqlite:main.sqlite3")
                    .await
                    .expect("Couldn't open database")
        )),
        s3: AmazonS3Builder::from_env()
            .with_endpoint("https://s3.us-east-005.backblazeb2.com")
            .with_region("s3.us-east-005")
            .with_bucket_name("kisslinux")
            .build()
            .expect("Couldn't create S3 state"),
    };

    let routes = generate_route_list(App);

    let app = Router::new()
        .route(
            "/api/{*fn_name}",
            get(server_fn_handler).post(server_fn_handler),
        )
        .leptos_routes_with_handler(routes, get(leptos_routes_handler))
        .fallback(leptos_axum::file_and_error_handler::<LeptosOptions, _>(shell))
        .with_state(state);

    // run our app with hyper
    // `axum::Server` is a re-export of `hyper::Server`
    log!("listening on http://{}", &addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}

#[cfg(not(feature = "ssr"))]
pub fn main() {
    // no client-side main function
    // unless we want this to work with e.g., Trunk for pure client-side testing
    // see lib.rs for hydration function instead
}
