-----------------------------------------------------------------------------------------------------------------------------

Explanation of derived versions based on Comfyui openai API 2026-05-13

-----------------------------------------------------------------------------------------------------------------------------

```markdown
# ComfyUI OpenAI API Proxy

A high-performance reverse proxy built in Rust that seamlessly translates standard OpenAI image/video generation API calls into requests for ComfyUI backends. Features multi-backend health checks, intelligent load balancing, WebSocket dual-channel communication, exponential backoff with full jitter, token bucket rate limiting, idempotency key caching, request-level response caching, and OpenTelemetry observability, providing production-grade reliability for generation services.

## Overview

`comfyui-openai-api` serves as a bridge between OpenAI API-compatible clients and the ComfyUI workflow engine. Its core responsibilities:

- **Receive** standard OpenAI-format image/video generation requests
- **Translate** request parameters into ComfyUI workflow injection format
- **Route** requests to healthy ComfyUI backends based on configured policies
- **Manage** asynchronous task lifecycles, supporting state persistence and querying
- **Deliver** responses conforming to the OpenAI API specification (base64-encoded images/videos)

Within the **LocalMiniDrama** (local AI short drama creation tool) ecosystem and similar projects, this proxy acts as the unified API foundation for the underlying generation engine, offering stable and scalable inference scheduling throughout the entire pipeline from script to final video.

## Core Features

### API Compatibility
- **OpenAI Image Generation** — `POST /v1/images/generations`, synchronous, returns base64 images
- **Video Generation Extension** — `POST /v1/videos/generations`, asynchronous, returns a `task_id`; query results via `GET /v1/tasks/{task_id}`
- **Task Lifecycle Management** — query, list, and delete tasks (`GET /v1/tasks`, `GET /v1/tasks/{task_id}`, `DELETE /v1/tasks/{task_id}`)
- **Model Listing** — `GET /v1/models` returns all available workflows (models)
- **Backend Status** — `GET /v1/backends` shows the health status of all backends
- **Health Check** — `GET /v1/health` liveness probe
- **Video Subsystem Status** — `GET /v1/videos/health`
- **Prometheus Metrics** — `GET /v1/metrics`
- **API Help Documentation** — `GET /v1/help`

### Multi-Backend Management
- Configure multiple ComfyUI backend instances; select explicitly via `?backend=xxx` or automatically
- **Periodic Health Checks**: Each backend is probed via `/system_stats`; if failures exceed a threshold the backend is automatically marked unhealthy and later reinstated upon recovery
- **Load Balancing Strategies**: Round Robin, Least Connections, Random — switchable via configuration

### Intelligent Workflow Injection
The proxy parses workflow JSON automatically, locating and injecting parameters based on node `class_type` and `_meta.title`:

| Node Type | Injected Parameter |
|-----------|-------------------|
| `CLIPTextEncode` (Positive) | Positive prompt `text` |
| `CLIPTextEncode` (Negative) | Negative prompt `text` |
| `EmptyLatentImage` etc. | `width`, `height`, `batch_size` |
| `LoadImage` | Reference image filename (auto-uploaded to ComfyUI) |
| `RandomNoise` / `KSampler` | `noise_seed` / `seed` |
| `PrimitiveInt` / `PrimitiveFloat` / `FloatSlider` | Duration, frame rate, dimensions, etc. |
| `PromptRelayEncode` | Multi-shot prompts and segment lengths |
| `LTXVAddGuideMulti` | Multi-reference guide frame indices |

### Reference Image Handling
- Supports both base64-embedded images and HTTP URL inputs
- Automatically uploads images to ComfyUI's `/upload/image` and injects them into `LoadImage` nodes
- When no reference images are provided, a 1×1 transparent placeholder is used to prevent workflow errors
- Dual-layer acceleration: in-memory cache + local filesystem cache (oldest half cleared when exceeding 1000 entries)

### Production Reliability
- **Exponential Backoff + Full Jitter**: ComfyUI history polling uses full jitter to avoid thundering herd effects
- **WebSocket Dual-Channel** (optional): Connects to the default backend for real-time completion notifications; automatically falls back to HTTP polling on disconnection
- **Token Bucket Rate Limiter**: Configurable; returns `429 Too Many Requests` when the limit is exceeded
- **Idempotency Key Support**: Through the `Idempotency-Key` request header, prevents duplicate submissions; cached responses survive for 24 hours
- **Request-Level Response Caching**: Same parameter combination (model + prompt hash + size + seed) hits the cache, returning results instantly
- **Graceful Shutdown**: On receiving SIGTERM, stops accepting new requests, waits for in-flight tasks to complete (or times out), then exits
- **Task Persistence**: All task states are stored in `tasks.json` and automatically restored upon service restart

### Observability
- **Prometheus Metrics** `/v1/metrics`: `total_requests`, `active_tasks`, `request_duration_seconds`, `cache_hit_total`
- **OpenTelemetry Distributed Tracing** (optional): Exports traces via OTLP to Jaeger/Tempo; each request is assigned a Trace ID
- **Structured Logging**: Base64 image data is automatically replaced with `[base64 omitted]`; long payloads are truncated
- **API Documentation** `/v1/help`: Returns detailed JSON documentation for all endpoints

### Seed Stability Tracking
- Specify a role name via the `X-Consistent-Role` request header; the proxy automatically tracks and reuses the last successful seed value for that role
- Ideal for multi-shot generation scenarios where character appearance must remain consistent (e.g., LocalMiniDrama's character consistency requirement)

## Quick Start

### Prerequisites
- Rust 1.70+
- A running ComfyUI backend (must be started with `--api`)
- (Optional) Docker and Docker Compose

### Installation Steps

**1. Clone the Repository**
```bash
git clone https://github.com/553556705-tech/comfyui-openai-api.git
cd comfyui-openai-api/apps/rust/comfyui-openai-api
```

**2. Create Configuration File**
```bash
cp config/config.sample.yaml config/config.yaml
```

**3. Edit `config/config.yaml`**

```yaml
log_level: "info"
server:
  host: "0.0.0.0"
  port: 8080

# Multi-backend list
comfyui_backends:
  - name: "backend-a"
    host: "127.0.0.1"
    port: 8000
    default: true
  - name: "backend-b"
    host: "192.168.50.16"
    port: 8000
    default: false

comfyui_backend:
  client_id: "comfyui-api"
  workflows_folder: "./workflows"
  use_ws: true               # set to false to disable WebSocket
  input_dir: "./cache"       # proxy local image cache directory

routing:
  timeout_seconds: 3600
  max_payload_size_mb: 500
  Image_Width: 1280
  Image_Height: 704
  video_Width: 1024
  video_Height: 576
  fps: 24
  free_model_before_video: true

  # Load balancing strategy (RoundRobin / LeastConnections / Random)
  lb_strategy: "RoundRobin"

  # Token bucket rate limiting (optional; comment out to disable)
  rate_limit:
    max_tokens: 60
    refill_rate: 1.0

  # Request-level response cache (optional; comment out to disable)
  response_cache:
    ttl_secs: 600
    max_entries: 500

  # Idempotency support
  enable_idempotency: true

  # Graceful shutdown timeout in seconds
  graceful_shutdown_timeout_secs: 30

  # Backend health check parameters
  health_check_interval_secs: 15
  health_check_fail_threshold: 3
```

**4. Prepare ComfyUI Workflow Files**

Place workflow JSON files into the `workflows/` directory. Ensure that key nodes have descriptive titles containing recognizable keywords (e.g., "Positive", "Negative", "Width", "Height", "Reference Image").

### Running

**Local Build**
```bash
cargo build --release
./target/release/comfyui-openai-api
```

**Docker**
```bash
docker build -t comfyui-openai-api .
docker run -p 8080:8080 \
  -v $(pwd)/config:/app/config \
  -v $(pwd)/workflows:/app/workflows \
  comfyui-openai-api
```

## API Endpoint Details

All endpoints use the `/v1` prefix. Below is the complete list:

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/v1/models` | GET | List all available models (workflows) |
| `/v1/health` | GET | Simple liveness check |
| `/v1/backends` | GET | Health status of all backends |
| `/v1/images/generations` | POST | Image generation (synchronous) |
| `/v1/videos/generations` | POST | Video generation (asynchronous) |
| `/v1/tasks` | GET | List all tasks |
| `/v1/tasks/{task_id}` | GET / DELETE | Query or delete a single task |
| `/v1/videos/health` | GET | Video subsystem status |
| `/v1/metrics` | GET | Prometheus metrics export |
| `/v1/help` | GET | API help documentation (JSON) |

### 1. Image Generation `POST /v1/images/generations`

**Request Example**
```bash
curl -X POST 'http://localhost:8080/v1/images/generations?backend=backend-a' \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "sdxl-workflow",
    "prompt": "a cat wearing a hat, masterpiece",
    "negative_prompt": "low quality, blurry",
    "size": "1024x1024",
    "n": 1,
    "seed": 42,
    "reference_images": [
      {"name": "ref1", "data": "data:image/png;base64,iVBOR..."}
    ]
  }'
```

**Request Body Parameters**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `model` | string | Yes | Workflow filename without `.json` extension |
| `prompt` | string | No | Positive prompt |
| `negative_prompt` | string | No | Negative prompt |
| `size` | string | No | Dimensions, e.g., `"1024x1024"` (overridable by config) |
| `seed` | integer | No | Random seed |
| `n` | integer | No | Number of images (batch size) |
| `reference_images` | array | No | Reference image array `[{name, data}]` |
| `image` | array | No | Array of base64 image strings (equivalent to `reference_images`) |

**Query Parameters**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `backend` | string | No | Specify backend name; load balancing used if omitted |

**Response Format**
```json
{
  "created": 1704067200,
  "data": [
    {
      "b64_json": "iVBORw0KGgoAAAANSUhEUg..."
    }
  ]
}
```

### 2. Video Generation `POST /v1/videos/generations`

**Request Example**
```bash
curl -X POST 'http://localhost:8080/v1/videos/generations?backend=backend-b' \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "video-workflow",
    "content": [
      {"type": "text", "text": "a dog running in the park"},
      {"type": "image_url", "image_url": {"url": "https://example.com/ref.png"}, "role": "reference_image"}
    ],
    "duration": 5,
    "resolution": "720p"
  }'
```

**Request Body Parameters**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `model` | string | Yes | Video workflow filename |
| `content` | array | Yes | Content array, items can be `{"type":"text", "text":"..."}` or `{"type":"image_url", "image_url":{"url":"..."}, "role":"reference_image"}` |
| `duration` | integer | No | Duration in seconds, default 5 |
| `resolution` | string | No | `"720p"` or `"1080p"` |
| `ratio` | string | No | Aspect ratio, e.g., `"16:9"` |
| `local_prompts` | string | No | Multi-shot prompts, format `[start-end]\ndescription` |
| `global_prompt` | string | No | Global prompt |
| `guide_strengths` | array | No | Guide strength array |

**Response Format**
```json
{
  "task_id": "vid-1715432100123-789"
}
```

### 3. Task Query `GET /v1/tasks/{task_id}`

```bash
curl http://localhost:8080/v1/tasks/vid-1715432100123-789
```

Response (processing):
```json
{
  "status": "processing"
}
```

Response (completed):
```json
{
  "status": "completed",
  "video_url": "http://backend-b:8000/view?filename=video.mp4&subfolder=&type=output",
  "b64_json": "..."
}
```

Response (failed):
```json
{
  "status": "failed",
  "error": "ComfyUI node error: ..."
}
```

### 4. List All Tasks `GET /v1/tasks`

```bash
curl http://localhost:8080/v1/tasks
```

Returns:
```json
{
  "tasks": [
    {
      "task_id": "vid-1715432100123-789",
      "status": "completed"
    },
    {
      "task_id": "img-1715432100456-123",
      "status": "processing"
    }
  ]
}
```

### 5. Delete Task `DELETE /v1/tasks/{task_id}`

```bash
curl -X DELETE http://localhost:8080/v1/tasks/img-1715432100456-123
```
Returns HTTP `204 No Content` on success.

### 6. Other Endpoints

- **List Models** `GET /v1/models`
```bash
curl http://localhost:8080/v1/models
```
Response:
```json
{
  "object": "list",
  "data": [
    { "id": "sdxl-workflow", "object": "model", "owned_by": "comfyui-openai-api" },
    { "id": "video-workflow", "object": "model", "owned_by": "comfyui-openai-api" }
  ]
}
```

- **Backend Health Status** `GET /v1/backends`
```json
{
  "backends": [
    { "name": "backend-a", "healthy": true },
    { "name": "backend-b", "healthy": false }
  ]
}
```

- **Liveness Probe** `GET /v1/health`: Returns `OK`

- **Prometheus Metrics** `GET /v1/metrics`: Returns standard Prometheus text format.

### Python Client Integration

```python
from openai import OpenAI

client = OpenAI(
    api_key="dummy-key",
    base_url="http://localhost:8080/v1"
)

response = client.images.generate(
    model="sdxl-workflow",
    prompt="a cat wearing a hat",
    size="1024x1024",
    n=1,
    response_format="b64_json",
    extra_query={"backend": "backend-a"}
)

print(response.data[0].b64_json)
```

## Workflow Management

### Automatic Injection Principle

Upon receiving a request, the proxy loads the corresponding workflow JSON and iterates over all nodes, identifying injection points based on node type and title:

- **CLIPTextEncode**: Nodes with "Positive" in the title receive the positive prompt; those with "Negative" receive the negative prompt.
- **EmptyLatentImage / EmptySD3LatentImage / EmptyFlux2LatentImage**: Injected with `width`, `height`, `batch_size`.
- **LoadImage**: Sorted by the sequence number in titles such as "Reference Image 1", "Reference Image 2"; the corresponding reference image filename (already uploaded to ComfyUI) is injected.
- **PrimitiveInt / INTConstant / PrimitiveFloat / FloatSlider**: Nodes titled "Width", "Height", "Duration", "FPS", etc., receive the corresponding numeric values.
- **RandomNoise / KSampler**: Injected with a unified random seed.
- **PromptRelayEncode**: `local_prompts` and `segment_lengths` are automatically generated (for multi-shot videos).
- **LTXVAddGuideMulti**: Guide frame indices are dynamically calculated based on the number of reference images.

### Creating Custom Workflows

1. Design your workflow in ComfyUI and set descriptive **titles** for key nodes:
   - Positive prompt CLIPTextEncode → title contains **"Positive"**
   - Negative prompt CLIPTextEncode → title contains **"Negative"**
   - Width integer/float node → **"Width"**
   - Height integer/float node → **"Height"**
   - FPS node → **"FPS"**
   - Duration node → **"Duration"**
   - Reference image LoadImage nodes → **"Reference Image 1"**, **"Reference Image 2"**, … (the sequence number determines injection order)
2. Export the workflow as a JSON file (the filename will be used as the `model` parameter).
3. Place the JSON file into the configured `workflows_folder`.
4. Start the proxy and call the workflow by specifying the `model` parameter.

## Configuration Reference

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `CONFIG_PATH` | Path to the YAML configuration file | `./config/config.yaml` |
| `RUST_LOG` | Log level (can be overridden by config file) | `info` |

### Complete Configuration Details

```yaml
# Log level: trace, debug, info, warn, error
log_level: "info"

# Proxy server binding address and port
server:
  host: "0.0.0.0"
  port: 8080

# Multi-backend list, at least one backend required
comfyui_backends:
  - name: "backend-a"        # Unique name used with ?backend=
    host: "127.0.0.1"        # ComfyUI address
    port: 8000               # ComfyUI port
    default: true            # Whether this is the default backend (for WebSocket)
  - name: "backend-b"
    host: "192.168.1.100"
    port: 8188
    default: false

# Internal proxy settings
comfyui_backend:
  client_id: "comfyui-api"         # WebSocket client ID
  workflows_folder: "./workflows"  # Directory for workflow JSON files
  use_ws: true                     # Enable WebSocket connection to default backend
  input_dir: "./cache"             # Image cache directory, stores uploaded reference images

# Routing and runtime configuration
routing:
  timeout_seconds: 3600            # Total timeout for ComfyUI tasks (seconds)
  max_payload_size_mb: 500         # Maximum request body size (MB)
  Image_Width: 1280                # Default image width (overridable by request size)
  Image_Height: 704                # Default image height
  video_Width: 1024                # Default video width
  video_Height: 576                # Default video height
  fps: 24                          # Default frame rate
  free_model_before_video: true    # Call ComfyUI /free before video generation to free VRAM

  # Load balancing strategy: RoundRobin, LeastConnections, Random
  lb_strategy: "RoundRobin"

  # Token bucket rate limiting (optional; comment out to disable)
  rate_limit:
    max_tokens: 60       # Bucket capacity
    refill_rate: 1.0     # Tokens refilled per second

  # Request-level response cache (optional; comment out to disable)
  response_cache:
    ttl_secs: 600        # Cache TTL (seconds)
    max_entries: 500     # Maximum LRU entries

  # Enable idempotency key checking (Idempotency-Key header)
  enable_idempotency: true

  # Graceful shutdown maximum wait time (seconds), force exit on timeout
  graceful_shutdown_timeout_secs: 30

  # Health check interval (seconds) and consecutive failure threshold
  health_check_interval_secs: 15
  health_check_fail_threshold: 3
```

## Project Structure

```
apps/rust/comfyui-openai-api/
├── Cargo.toml
├── build.sh
├── Dockerfile
├── tasks.json                 # Task persistence file (auto-generated; should be .gitignored)
├── config/
│   ├── config.yaml            # Runtime configuration (not committed to repository)
│   └── config_sample.yaml     # Configuration template
├── workflows/                 # Sample workflows (recommend .gitignoring the whole directory or specific files)
│   └── *.json
└── src/
    ├── main.rs                # Service entry point, route registration and middleware
    ├── config.rs              # Configuration structures and YAML parsing
    ├── proxy.rs               # Global shared state (ProxyState)
    ├── error.rs               # Unified error handling (OpenAI-style error format)
    ├── seed_tracker.rs        # Character seed stability tracker
    ├── graceful.rs            # Graceful shutdown (task draining)
    ├── tracing_setup.rs       # OpenTelemetry initialization
    ├── task_manager.rs        # Task persistence, CRUD operations
    ├── workflows/
    │   ├── mod.rs
    │   ├── registry.rs        # Workflow registry, model listing
    │   └── template.rs        # Pre-parsed injection point mapping (PreparedWorkflow)
    ├── handlers/
    │   ├── mod.rs
    │   ├── image.rs           # /v1/images/generations image generation
    │   ├── video.rs           # /v1/videos/generations video generation
    │   ├── tasks.rs           # /v1/tasks series
    │   ├── metrics.rs         # /v1/metrics Prometheus metrics
    │   ├── models.rs          # /v1/models model listing
    │   ├── health.rs          # /v1/health health check
    │   └── backends.rs        # /v1/backends backend status
    ├── backend/
    │   ├── mod.rs
    │   ├── pool.rs            # Backend connection pool, health checks, load balancing
    │   └── router.rs          # Routing strategy placeholder (extensible)
    ├── transport/
    │   ├── mod.rs
    │   ├── poll.rs            # HTTP history polling (full jitter backoff)
    │   └── ws.rs              # WebSocket real-time notification manager
    ├── middleware/
    │   ├── mod.rs
    │   ├── rate_limiter.rs    # Token bucket rate limiter
    │   ├── request_id.rs      # Request ID middleware
    │   └── idempotency.rs     # Idempotency key storage and checking
    └── cache/
        ├── mod.rs
        ├── image_cache.rs     # Local image caching and automatic upload
        └── response_cache.rs  # LRU request-level response cache
```

## Architecture and Request Flow

### Architectural Overview

```
┌──────────────────────────────────────┐
│        OpenAI Compatible Client      │
│   (Python, JS, curl, LocalMiniDrama) │
└────────────────┬─────────────────────┘
                 │ HTTP POST /v1/images/generations
                 ▼
┌──────────────────────────────────────┐
│        comfyui-openai-api            │
│  ┌──────────┐ ┌────────────────┐     │
│  │ Rate     │ │ Idempotency    │     │
│  │ Limiter  │ │ Check          │     │
│  └──────────┘ └────────────────┘     │
│  ┌──────────┐ ┌────────────────┐     │
│  │ Workflow │ │ Backend Pool   │     │
│  │ Injector │ │ & LB + Health  │     │
│  └──────────┘ └────────────────┘     │
│  ┌──────────────────────────────┐    │
│  │ WebSocket / HTTP Polling     │    │
│  │ (full jitter backoff)        │    │
│  └──────────────────────────────┘    │
└────────────────┬─────────────────────┘
                 │
                 ▼
┌──────────────────────────────────────┐
│  ComfyUI Backend A (healthy)         │
│  ComfyUI Backend B (healthy)         │
│  ComfyUI Backend C (unhealthy, off)  │
└──────────────────────────────────────┘
```

### Complete Image Request Lifecycle

```
Client POST /v1/images/generations?backend=xxx
    │
    ▼
[1] Graceful shutdown check          → if shutting down, return 500
[2] Idempotency check (optional)     → if hit, return cached response directly
[3] Token bucket rate limit (opt.)   → if exceeded, return 429
[4] Backend selection                → explicit query string or load balancing
[5] Workflow template loading        → fetch PreparedWorkflow from WorkflowRegistry
[6] Request translation & injection  → seed, prompt, size, reference images, etc.
[7] Submission to ComfyUI            → POST /prompt, obtain prompt_id
[8] Wait for completion              → preferably via WebSocket, fallback to HTTP polling (full jitter)
[9] Image download                   → /view download → base64 encode
[10] OpenAI-format response          → {"created":..., "data":[{"b64_json":"..."}]}
[11] Cache write (optional)          → request-level response cache + idempotency key cache
[12] Seed tracking update (opt.)     → update successful seed for the given role
```

## Performance Characteristics

| Feature | Description |
|---------|-------------|
| Async I/O | Based on the Tokio runtime, non-blocking concurrent request handling |
| Connection Reuse | Reqwest HTTP client with TCP_NODELAY and connection pooling |
| Polling Optimization | Exponential backoff + full jitter to avoid thundering herd |
| Memory Control | Image cache auto-clears half when exceeding 1000 entries; configurable LRU response cache |
| Task Persistence | HashMap + `tasks.json`, survives restarts |
| Configurable Timeouts | Global request timeout, polling timeout, graceful shutdown timeout independently settable |
| Request Size Limit | `max_payload_size_mb` prevents oversized requests |

## Monitoring and Debugging

### Log Control

Control via config file `log_level` or the `RUST_LOG` environment variable:

| Level | Output Content |
|-------|----------------|
| `trace` | Full request/response bodies, workflow JSON details |
| `debug` | Critical path logs, WebSocket status |
| `info` | Request ingress, backend selection, task completion (default) |
| `warn` | Retries, fallbacks, recoverable errors |
| `error` | Connection failures, task exceptions |

### Prometheus Metrics (`/v1/metrics`)

| Metric Name | Type | Description |
|-------------|------|-------------|
| `total_requests` | Counter | Total number of requests |
| `active_tasks` | Gauge | Currently active tasks |
| `request_duration_seconds` | Histogram | End-to-end request latency distribution |
| `cache_hit_total` | Counter | Response cache hit count |

### Distributed Tracing

When OpenTelemetry is enabled, the proxy exports traces via OTLP to backends like Jaeger/Tempo. Each request receives a globally unique `x-request-id`, and key processing stages are wrapped in spans.

### Built-in Task Dashboard

Every 5 seconds a summary of current tasks is logged:

```
📋 Task status (3 total):
   🟡 img-1715432100123-456 - Processing
   🟢 vid-1715432100456-789 - Completed
   🔴 img-1715432100789-012 - Failed: ComfyUI error: ...
```

## Ecosystem Integration with LocalMiniDrama

- **Unified Generation Interface**: `LocalMiniDrama` calls this proxy through the standard OpenAI API, without needing to understand ComfyUI workflow details.
- **Batch Shot Generation**: Multi-shot generation can be accelerated by leveraging the proxy's multi-backend load balancing.
- **Character Consistency**: The `X-Consistent-Role` header works with the seed tracker to keep the same character's appearance consistent across multiple shots.
- **Video Model Support**: Built-in injection compatibility for models like Doubao Seedance, Tongyi Wanxiang, Vidu, etc., covering various video generation needs in short drama production.

## Troubleshooting

| Symptom | Possible Cause | Troubleshooting |
|---------|---------------|-----------------|
| 502 Bad Gateway | ComfyUI backend unreachable | Verify host/port in `comfyui_backends` config; ensure ComfyUI is running with `--api` |
| 404 Workflow not found | Workflow file missing for the `model` parameter | Check that `{model}.json` exists in the `workflows_folder` |
| 400 Invalid request | Malformed request body or base64 decode failure | Validate JSON format and base64 encoding |
| 504 Timeout | Generation took longer than `timeout_seconds` | Increase timeout value or inspect ComfyUI node errors |
| 429 Too Many Requests | Token bucket rate limit triggered | Adjust `rate_limit` config or reduce request frequency |
| Backend removed | Health checks failing consecutively | Check that ComfyUI's `/system_stats` is responding normally and network connectivity is intact |

## Version History

### v0.3.0
- 🏗️ Modular architecture refactoring, splitting into handlers/backend/transport/middleware/cache/workflows
- 🔄 Multi-backend health checks and load balancing (RoundRobin / LeastConnections / Random)
- 🔒 Token bucket rate limiter middleware
- 🆔 Idempotency key support
- 💾 Request-level LRU response cache
- 📡 OpenTelemetry distributed tracing
- 🧹 Graceful shutdown with task draining
- 🌱 Character seed stability tracker
- 📋 New endpoints: `/v1/models`, `/v1/backends`, `/v1/tasks` list and delete

### v0.2.0
- Video generation support
- Multi-backend manual routing
- Task persistence (tasks.json)
- PromptRelayEncode / LTXVAddGuideMulti node injection

### v0.1.0
- Initial release: OpenAI image generation compatibility

## Contributing

Contributions are welcome via Issue and Pull Request. Please follow these guidelines:

- Add documentation comments for new public functions and modules
- User-facing changes must update configuration samples and API documentation (this README and the /v1/help endpoint)
- Test against various workflow configurations
- Follow the existing code style and module organization

## License

MIT License

---

*This project is part of the [LocalMiniDrama](https://github.com/553556705-tech/LocalMiniDrama.git) ecosystem, providing a stable and scalable underlying generation engine API foundation for local AI short drama creation.*
```
