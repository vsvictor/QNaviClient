use anyhow::{Context, Result};
use bytes::{Bytes, BytesMut, Buf};
use h3::client;
use h3_quinn::{Connection as H3QuinnConnection, OpenStreams as H3QuinnOpenStreams};
use http::{Method, Request};
use quinn::{ClientConfig, Connection, Endpoint};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

#[derive(Serialize)]
struct RegisterRequest<'a> {
    username: &'a str,
    password: &'a str,
    email: Option<&'a str>,
}

#[derive(Serialize)]
struct LoginRequest<'a> {
    username: &'a str,
    password: &'a str,
}

#[derive(Serialize)]
struct RefreshRequest<'a> {
    refresh_token: &'a str,
}

#[derive(Deserialize, Debug, Clone)]
struct AuthResponse {
    access_token: Option<String>,   // renamed from `token` to `access_token`
    refresh_token: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize, Debug)]
struct Profile {
    id: String,
    username: String,
    email: Option<String>,
}

async fn make_quic_h3_connection(
    server_addr: &str,
    server_name: &str,
) -> Result<(
    client::Connection<H3QuinnConnection, Bytes>,
    client::SendRequest<H3QuinnOpenStreams, Bytes>,
)> {
    // Create a QUIC endpoint bound to any available UDP port
    let mut endpoint = Endpoint::client("0.0.0.0:0".parse().unwrap())?;

    // Use platform verifier (quinn 0.11). Adjust if your platform requires another factory.
    let client_cfg = ClientConfig::try_with_platform_verifier()
        .context("failed to build client config with platform verifier")?;
    endpoint.set_default_client_config(client_cfg);

    // Connect
    let connecting = endpoint.connect(server_addr.parse()?, server_name)?;
    let new_conn: Connection = connecting.await.context("failed to connect")?;

    info!("connected to {}", new_conn.remote_address());

    // Wrap quinn::Connection into h3_quinn adapter
    let h3_quinn_conn = H3QuinnConnection::new(new_conn);

    // Create h3 client over the adapted connection.
    // client::new returns (Connection<C, Bytes>, SendRequest<C::OpenStreams, Bytes>)
    let (h3_conn, send_request) = client::new(h3_quinn_conn).await.context("create h3 client")?;

    Ok((h3_conn, send_request))
}

// НАЙШВИДШИЙ ЕКСПЕРИМЕНТ:
// Читання тіла відповіді безпосередньо з RequestStream через recv_data()
// (Після виклику recv_response() беремо заголовки/статус, а самі байти читаємо через req_stream.recv_data())
// Тепер функції приймають base_url (наприклад "https://mobilespace.dev") і будують абсолютний URI.
// Також додано optional auth header (Authorization: Bearer ...)
async fn send_json_request<Rq: Serialize, Rs: for<'de> Deserialize<'de>>(
    sender: &mut client::SendRequest<H3QuinnOpenStreams, Bytes>,
    method: Method,
    base_url: &str,
    path: &str,
    body: &Rq,
    auth: Option<&str>,
) -> Result<Rs> {
    let json = serde_json::to_vec(body)?;
    // Build absolute URI
    let uri = format!("{}{}", base_url.trim_end_matches('/'), path);

    // Build request using the http crate types resolved by Cargo
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");

    if let Some(auth_val) = auth {
        builder = builder.header("authorization", auth_val);
    }

    let req: Request<()> = builder.body(()).unwrap();

    // Obtain the RequestStream from h3 send_request
    let mut req_stream = sender.send_request(req).await?;

    // Send request body and finish the send-side
    req_stream.send_data(Bytes::from(json)).await?;
    req_stream.finish().await?;

    // Receive response headers/status (we ignore the body via this call if it returns Response or similar)
    let _response = req_stream.recv_response().await?;

    // Read body directly from the RequestStream using recv_data() API
    let mut body_buf = BytesMut::new();
    loop {
        match req_stream.recv_data().await {
            Ok(Some(mut chunk_buf)) => {
                // chunk_buf implements bytes::Buf — consume it via chunk() / advance()
                while chunk_buf.remaining() > 0 {
                    let c = chunk_buf.chunk();
                    if c.is_empty() {
                        break;
                    }
                    body_buf.extend_from_slice(c);
                    let n = c.len();
                    chunk_buf.advance(n);
                }
            }
            Ok(None) => break, // end of body
            Err(e) => return Err(e.into()),
        }
    }

    // Deserialize from the accumulated buffer
    let parsed: Rs = serde_json::from_slice(&body_buf)?;
    Ok(parsed)
}

// Аналогічно для GET (без тіла при відправленні). Додаємо optional auth header.
async fn get_json_request<Rs: for<'de> Deserialize<'de>>(
    sender: &mut client::SendRequest<H3QuinnOpenStreams, Bytes>,
    method: Method,
    base_url: &str,
    path: &str,
    auth: Option<&str>,
) -> Result<Rs> {
    let uri = format!("{}{}", base_url.trim_end_matches('/'), path);
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("accept", "application/json");

    if let Some(auth_val) = auth {
        builder = builder.header("authorization", auth_val);
    }

    let req: Request<()> = builder.body(()).unwrap();

    let mut req_stream = sender.send_request(req).await?;
    req_stream.finish().await?;

    // Receive headers/status
    let _response = req_stream.recv_response().await?;

    // Read body from the RequestStream
    let mut body_buf = BytesMut::new();
    loop {
        match req_stream.recv_data().await {
            Ok(Some(mut chunk_buf)) => {
                while chunk_buf.remaining() > 0 {
                    let c = chunk_buf.chunk();
                    if c.is_empty() {
                        break;
                    }
                    body_buf.extend_from_slice(c);
                    let n = c.len();
                    chunk_buf.advance(n);
                }
            }
            Ok(None) => break,
            Err(e) => return Err(e.into()),
        }
    }

    let parsed: Rs = serde_json::from_slice(&body_buf)?;
    Ok(parsed)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // Configure your server address & SNI
    let server_addr = "192.168.0.44:4433"; // UDP:PORT
    let server_name = "mobilespace.dev"; // SNI
    let base_url = format!("https://{}", server_name);

    let (mut _conn, mut sender) = make_quic_h3_connection(server_addr, &server_name).await?;

    // Example calls
    let reg = RegisterRequest {
        username: "alice",
        password: "pass",
        email: Some("alice@example.com"),
    };
    match send_json_request::<_, AuthResponse>(
        &mut sender,
        Method::POST,
        &base_url,
        "/register",
        &reg,
        None,
    )
        .await
    {
        Ok(r) => info!("register: {:?}", r),
        Err(e) => warn!("register error: {:?}", e),
    }

    let login = LoginRequest {
        username: "alice",
        password: "pass",
    };
    // store tokens here
    let mut access_token = None::<String>;
    let mut refresh_token = None::<String>;
    match send_json_request::<_, AuthResponse>(&mut sender, Method::POST, &base_url, "/login", &login, None)
        .await
    {
        Ok(r) => {
            info!("login: {:?}", r);
            // prefer `access_token` field name (server uses access_token)
            if let Some(t) = r.access_token {
                access_token = Some(t);
            }
            if let Some(rt) = r.refresh_token {
                refresh_token = Some(rt);
            }
        }
        Err(e) => warn!("login error: {:?}", e),
    }

    // If we have a refresh token, call /refresh (and update tokens if returned)
    if let Some(ref rt) = refresh_token {
        let refresh_req = RefreshRequest { refresh_token: rt.as_str() };
        match send_json_request::<_, AuthResponse>(&mut sender, Method::POST, &base_url, "/refresh", &refresh_req, None).await {
            Ok(r) => {
                info!("refresh: {:?}", r);
                // update tokens if returned
                if let Some(t) = r.access_token {
                    access_token = Some(t);
                }
                if let Some(nrt) = r.refresh_token {
                    refresh_token = Some(nrt);
                }
            }
            Err(e) => warn!("refresh error: {:?}", e),
        }
    } else {
        warn!("No refresh token available after login; skipping refresh request.");
    }

    // --- ADD 2 SECOND DELAY BEFORE GET /profile ---
    // Wait 2 seconds before requesting profile
    //sleep(Duration::from_secs(2)).await;

    // GET /profile using Authorization header if access_token exists
    if let Some(ref at) = access_token {
        let auth_header = format!("Bearer {}", at);
        match get_json_request::<Profile>(&mut sender, Method::GET, &base_url, "/profile", Some(&auth_header)).await {
            Ok(p) => info!("profile: {:?}", p),
            Err(e) => warn!("profile error: {:?}", e),
        }
    } else {
        // try without auth (depends on API)
        match get_json_request::<Profile>(&mut sender, Method::GET, &base_url, "/profile", None).await {
            Ok(p) => info!("profile (no auth): {:?}", p),
            Err(e) => warn!("profile error (no auth): {:?}", e),
        }
    }

    // Logout: call POST /logout with refresh token in body if available; include Authorization if access token available
    if refresh_token.is_some() || access_token.is_some() {
        let logout_body = match refresh_token.as_deref() {
            Some(rtok) => serde_json::json!({ "refresh_token": rtok }),
            None => serde_json::json!({}),
        };
        let auth_header_opt = access_token.as_deref().map(|t| format!("Bearer {}", t));

        match send_json_request::<_, AuthResponse>(
            &mut sender,
            Method::POST,
            &base_url,
            "/logout",
            &logout_body,
            auth_header_opt.as_deref(),
        )
            .await
        {
            Ok(r) => info!("logout: {:?}", r),
            Err(e) => warn!("logout error: {:?}", e),
        }
    } else {
        info!("No tokens available; skipping logout request.");
    }

    // Wait for Ctrl-C before exiting so user can inspect logs/connection if needed.
    info!("Finished requests; press Ctrl-C to exit.");
    // Wait for SIGINT (Ctrl-C)
    tokio::signal::ctrl_c().await.context("failed to listen for ctrl-c")?;

    info!("Shutdown requested via Ctrl-C; exiting.");
    Ok(())
}