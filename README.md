# oneclaw-runtime

Rust library defining the `RuntimeProvider` trait and implementations for managing containerized AI agent runtimes on [1Claw](https://1claw.xyz).

## Providers

| Provider | Backend | Use Case |
|----------|---------|----------|
| `LocalDockerProvider` | Docker | Local development via `1claw spawn` / `1claw init` |
| `GkeProvider` | Google Kubernetes Engine | Production cloud runtimes with gVisor isolation |
| `CloudRunProvider` | Google Cloud Run | Serverless scale-to-zero runtimes |

## Usage

```rust
use oneclaw_runtime::{RuntimeProvider, RuntimeSpec, Preset};
use oneclaw_runtime::providers::docker::{LocalDockerProvider, DockerConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = LocalDockerProvider::new(DockerConfig::default());

    let spec = RuntimeSpec::builder()
        .name("my-agent")
        .image("python:3.12-slim")
        .preset(Preset::Medium)
        .env("MODEL", "gpt-4o")
        .expose_http(true)
        .slug("my-agent")
        .build();

    let handle = provider.create(spec).await?;
    println!("Runtime {} is {:?}", handle.id, handle.status);

    let logs = provider.logs(&handle.id, Default::default()).await?;
    for entry in logs {
        println!("[{:?}] {}", entry.stream, entry.message);
    }

    provider.stop(&handle.id).await?;
    provider.delete(&handle.id).await?;
    Ok(())
}
```

## Features

Enable providers via Cargo features:

```toml
[dependencies]
oneclaw-runtime = { version = "0.1", features = ["docker"] }        # Local only (default)
oneclaw-runtime = { version = "0.1", features = ["gke"] }           # GKE production
oneclaw-runtime = { version = "0.1", features = ["all-providers"] } # Everything
```

## Security

The `LocalDockerProvider` applies hardened defaults:

- `--cap-drop ALL` — no Linux capabilities
- `--security-opt no-new-privileges`
- Read-only rootfs with tmpfs for `/tmp` and `/secrets`
- Non-root user (65534:65534)
- PID limit (256)
- CPU/memory limits per preset
- Isolated Docker network (no internet by default)
- **No Docker socket mounting — ever**

## License

MIT
