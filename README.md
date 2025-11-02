```markdown
# QNaviClient

Native QUIC/HTTP3 client for QNavi (macOS). Binary name: QNaviClient

## Build (macOS)

Install build deps for quiche if not already:
```bash
brew install pkg-config cmake ninja
brew install openssl@3    # if needed
export OPENSSL_DIR="$(brew --prefix openssl@3)"
export PKG_CONFIG_PATH="$OPENSSL_DIR/lib/pkgconfig:$PKG_CONFIG_PATH"
```

Build release:
```bash
cargo build --release
```

The produced binary will be:
- ./target/release/QNaviClient

(If you prefer lowercase executable, change package.name in Cargo.toml or add a [[bin]] entry.)

## Usage

Basic options:
- --addr HOST:PORT (default 127.0.0.1:4433)
- --timeout N (seconds, default 10)

Examples:

Register user:
```bash
./target/release/QNaviClient --addr 127.0.0.1:4433 register alice s3cr3t
# or during development:
cargo run --release -- --addr 127.0.0.1:4433 register alice s3cr3t
```

Login:
```bash
./target/release/QNaviClient --addr 127.0.0.1:4433 login alice s3cr3t
```

Refresh:
```bash
./target/release/QNaviClient --addr 127.0.0.1:4433 refresh "<refresh_token>"
```

Get profile:
```bash
./target/release/QNaviClient --addr 127.0.0.1:4433 getprofile "<access_token>"
```

### Quick shell example (save access/refresh tokens)
```bash
RESP=$(./target/release/QNaviClient --addr 127.0.0.1:4433 login alice s3cr3t | sed -n '2,$p')
ACCESS_TOKEN=$(echo "$RESP" | jq -r '.access_token')
REFRESH_TOKEN=$(echo "$RESP" | jq -r '.refresh_token')
```

## Notes
- For local testing with self-signed certs the client disables peer verification (in code: `config.verify_peer(false)`). Do not use this in production.
- If handshake fails, increase `--timeout` or verify server is up and reachable.
- If quiche fails to compile, ensure system build tools and OPENSSL_DIR / PKG_CONFIG_PATH environment are set as above.
```