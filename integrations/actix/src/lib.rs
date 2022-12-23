use actix_web::{http::header::HeaderMap, web::Bytes, *};
use futures::{StreamExt, executor}; 

use http::StatusCode;
use leptos::*;
use leptos_meta::*;
use leptos_router::*;
use std::sync::Arc;
use tokio::sync::RwLock;

/// This struct lets you define headers and override the status of the Response from an Element or a Server Function
/// Typically contained inside of a ResponseOptions. Setting this is useful for cookies and custom responses.
#[derive(Debug, Clone, Default)]
pub struct ResponseParts {
    pub headers: HeaderMap,
    pub status: Option<StatusCode>,
}

/// Adding this Struct to your Scope inside of a Server Fn or Elements will allow you to override details of the Response
/// like StatusCode and add Headers/Cookies. Because Elements and Server Fns are lower in the tree than the Response generation
/// code, it needs to be wrapped in an `Arc<RwLock<>>` so that it can be surfaced
#[derive(Debug, Clone, Default)]
pub struct ResponseOptions(pub Arc<RwLock<ResponseParts>>);

impl ResponseOptions {
    /// A less boilerplatey way to overwrite the contents of `ResponseOptions` with a new `ResponseParts`
    pub async fn overwrite(&self, parts: ResponseParts) {
        let mut writable = self.0.write().await;
        *writable = parts
    }
}

/// An Actix [Route](actix_web::Route) that listens for a `POST` request with
/// Leptos server function arguments in the body, runs the server function if found,
/// and returns the resulting [HttpResponse].
///
/// This provides the [HttpRequest] to the server [Scope](leptos::Scope).
///
/// This can then be set up at an appropriate route in your application:
///
/// ```
/// use actix_web::*;
///
/// fn register_server_functions() {
///   // call ServerFn::register() for each of the server functions you've defined
/// }
///
/// # if false { // don't actually try to run a server in a doctest...
/// #[actix_web::main]
/// async fn main() -> std::io::Result<()> {
///     // make sure you actually register your server functions
///     register_server_functions();
///
///     HttpServer::new(|| {
///         App::new()
///             // "/api" should match the prefix, if any, declared when defining server functions
///             // {tail:.*} passes the remainder of the URL as the server function name
///             .route("/api/{tail:.*}", leptos_actix::handle_server_fns())
///     })
///     .bind(("127.0.0.1", 8080))?
///     .run()
///     .await
/// }
/// # }
/// ```
pub fn handle_server_fns() -> Route {
    web::post().to(
        |req: HttpRequest, params: web::Path<String>, body: web::Bytes| async move {
            {
                let path = params.into_inner();
                let accept_header = req
                    .headers()
                    .get("Accept")
                    .and_then(|value| value.to_str().ok());

                if let Some(server_fn) = server_fn_by_path(path.as_str()) {
                    let body: &[u8] = &body;

                    let runtime = create_runtime();
                    let (cx, disposer) = raw_scope_and_disposer(runtime);
                    let res_options = ResponseOptions::default();

                    // provide HttpRequest as context in server scope
                    provide_context(cx, req.clone());
                    provide_context(cx, res_options.clone());

                    match server_fn(cx, body).await {
                        Ok(serialized) => {
                            let res_options = use_context::<ResponseOptions>(cx).unwrap();

                            // clean up the scope, which we only needed to run the server fn
                            disposer.dispose();
                            runtime.dispose();

                            let mut res: HttpResponseBuilder;
                            let mut res_parts = res_options.0.write().await;
                            // let (status, mut res_headers) = match res_parts {
                            //     Some(parts) => (parts.status, parts.headers),
                            //     None => (None, HeaderMap::new()),
                            // };

                            if accept_header == Some("application/json")
                                || accept_header == Some("application/x-www-form-urlencoded")
                                || accept_header == Some("application/cbor")
                            {
                                res = HttpResponse::Ok();

                                // Override Status if Status is set in ResponseParts and
                                // We're not trying to do a form submit
                                if let Some(status) = res_parts.status {
                                    res.status(status);
                                }
                            }
                            // otherwise, it's probably a <form> submit or something: redirect back to the referrer
                            else {
                                let referer = req
                                    .headers()
                                    .get("Referer")
                                    .and_then(|value| value.to_str().ok())
                                    .unwrap_or("/");
                                res = HttpResponse::SeeOther();
                                res.insert_header(("Location", referer))
                                    .content_type("application/json");
                            };
                            // Use provided ResponseParts headers if they exist
                            let _count = res_parts
                                .headers
                                .drain()
                                .map(|(k, v)| {
                                    if let Some(k) = k {
                                        res.insert_header((k, v));
                                    }
                                })
                                .count();

                            match serialized {
                                Payload::Binary(data) => {
                                    res.content_type("application/cbor");
                                    res.body(Bytes::from(data))
                                }
                                Payload::Url(data) => {
                                    res.content_type("application/x-www-form-urlencoded");
                                    res.body(data)
                                }
                                Payload::Json(data) => {
                                    res.content_type("application/json");
                                    res.body(data)
                                }
                            }
                        }
                        Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
                    }
                } else {
                    HttpResponse::BadRequest()
                        .body(format!("Could not find a server function at that route."))
                }
            }
        },
    )
}

/// Returns an Actix [Route](actix_web::Route) that listens for a `GET` request and tries
/// to route it using [leptos_router], serving an HTML stream of your application.
///
/// The provides a [MetaContext] and a [RouterIntegrationContext] to app’s context before
/// rendering it, and includes any meta tags injected using [leptos_meta].
///
/// The HTML stream is rendered using [render_to_stream], and includes everything described in
/// the documentation for that function.
///
/// This can then be set up at an appropriate route in your application:
/// ```
/// use actix_web::{HttpServer, App};
/// use leptos::*;
/// use std::{env,net::SocketAddr};
///
/// #[component]
/// fn MyApp(cx: Scope) -> Element {
///   view! { cx, <main>"Hello, world!"</main> }
/// }
///
/// # if false { // don't actually try to run a server in a doctest...
/// #[actix_web::main]
/// async fn main() -> std::io::Result<()> {
///
///     let addr = SocketAddr::from(([127,0,0,1],3000));
///     HttpServer::new(move || {
///         let render_options: RenderOptions = RenderOptions::builder().pkg_path("/pkg/leptos_example").reload_port(3001).socket_address(addr.clone()).environment(&env::var("RUST_ENV")).build();
///         render_options.write_to_file();
///         App::new()
///             // {tail:.*} passes the remainder of the URL as the route
///             // the actual routing will be handled by `leptos_router`
///             .route("/{tail:.*}", leptos_actix::render_app_to_stream(render_options, |cx| view! { cx, <MyApp/> }))
///     })
///     .bind(&addr)?
///     .run()
///     .await
/// }
/// # }
/// ```
pub fn render_app_to_stream(
    options: RenderOptions,
    app_fn: impl Fn(leptos::Scope) -> Element + Clone + 'static,
) -> Route {
    web::get().to(move |req: HttpRequest| {
        let options = options.clone();
        let app_fn = app_fn.clone();
        let res_options = ResponseOptions::default();
        let res_options_default = res_options.clone();
        async move {
            let path = req.path();

            let query = req.query_string();
            let path = if query.is_empty() {
                "http://leptos".to_string() + path
            } else {
                "http://leptos".to_string() + path + "?" + query
            };

            let app = {
                let app_fn = app_fn.clone();
                move |cx| {
                    let integration = ServerIntegration { path: path.clone() };
                    provide_context(cx, RouterIntegrationContext::new(integration));
                    provide_context(cx, MetaContext::new());
                    println!("Setting Default options");
                    provide_context(cx, res_options_default.clone());
                    provide_context(cx, req.clone());

                    (app_fn)(cx)
                }
            };

            let pkg_path = &options.pkg_path;
            let socket_ip = &options.socket_address.ip().to_string();
            let reload_port = options.reload_port;

            let leptos_autoreload = match options.environment {
                RustEnv::DEV => format!(
                    r#"
                        <script crossorigin="">(function () {{
                            var ws = new WebSocket('ws://{socket_ip}:{reload_port}/autoreload');
                            ws.onmessage = (ev) => {{
                                console.log(`Reload message: `);
                                if (ev.data === 'reload') window.location.reload();
                            }};
                            ws.onclose = () => console.warn('Autoreload stopped. Manual reload necessary.');
                        }})()
                        </script>
                    "#
                ),
                RustEnv::PROD => "".to_string(),
            };

            let head = format!(
                r#"<!DOCTYPE html>
                <html lang="en">
                    <head>
                        <meta charset="utf-8"/>
                        <meta name="viewport" content="width=device-width, initial-scale=1"/>
                        <link rel="modulepreload" href="{pkg_path}.js">
                        <link rel="preload" href="{pkg_path}_bg.wasm" as="fetch" type="application/wasm" crossorigin="">
                        <script type="module">import init, {{ hydrate }} from '{pkg_path}.js'; init('{pkg_path}_bg.wasm').then(hydrate);</script>
                        {leptos_autoreload}
                        "#
            );

            let tail = "</body></html>";

            let mut stream = Box::pin(futures::stream::once(async move { head.clone() }) 
            // TODO this leaks a runtime once per invocation
            .chain(render_to_stream(move |cx| {
                let app = app(cx);
                let head = use_context::<MetaContext>(cx)
                    .map(|meta| meta.dehydrate())
                    .unwrap_or_default();
                format!("{head}</head><body>{app}")
            }))
                .chain(futures::stream::once(async { tail.to_string() }))
                .map(|html| Ok(web::Bytes::from(html)) as Result<web::Bytes>));

            // Get the first, second, and third chunks in the stream, which renders the app shell, and thus allows Resources to run
            let first_chunk = stream.next().await;
            let second_chunk = stream.next().await;
            let third_chunk = stream.next().await;

            let res_options = res_options.0.read().await;
            println!("Reading Options");
            println!("Response Options: {:#?}", res_options);
            let (status, mut headers) = (res_options.status.clone(), res_options.headers.clone());
            let status = status.unwrap_or_default();
            
            let complete_stream =
            futures::stream::iter([first_chunk.unwrap(), second_chunk.unwrap(), third_chunk.unwrap()])
                .chain(stream);
            let mut res = HttpResponse::Ok().content_type("text/html").streaming(
                complete_stream
            );
            // Add headers manipulated in the response
            for (key, value) in headers.drain(){
                if let Some(key) = key{
                res.headers_mut().append(key, value);
                }
            };
            // Set status to what is returned in the function
            let res_status = res.status_mut();
            *res_status = status;
            // Return the response
            res

        }
    })
}