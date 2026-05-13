```markdown
# ComfyUI OpenAI API Proxy

基于 Rust 构建的高性能反向代理，将标准 OpenAI 图像/视频生成 API 调用无缝转换为 ComfyUI 后端请求。支持多后端健康检查、智能负载均衡、WebSocket 双通道、指数退避完全抖动、令牌桶限流、幂等键缓存、请求级响应缓存以及 OpenTelemetry 可观测性，为生成服务提供生产级可靠性。

## 概述

`comfyui-openai-api` 是 OpenAI API 兼容客户端与 ComfyUI 工作流引擎之间的桥梁，核心职责：

- **接收** 标准 OpenAI 格式的图像/视频生成请求
- **转换** 请求参数为 ComfyUI 工作流注入格式
- **路由** 根据配置策略将请求分发至健康的 ComfyUI 后端
- **管理** 异步任务生命周期，支持状态持久化与查询
- **交付** 符合 OpenAI API 规范的响应（Base64 编码图像/视频）

在 **LocalMiniDrama**（本地 AI 短剧全流程创作工具）等项目生态中，本代理承载着底层生成引擎的统一 API 基座角色，为从剧本到成片的完整链路提供稳定、可扩展的推理调度能力。

## 核心特性

### API 兼容性
- **OpenAI 图像生成** —— `POST /v1/images/generations`，同步返回 Base64 图像
- **视频生成扩展** —— `POST /v1/videos/generations`，异步返回 `task_id`，通过 `GET /v1/tasks/{task_id}` 查询结果
- **任务生命周期管理** —— 查询、列出、删除任务（`GET /v1/tasks`、`GET /v1/tasks/{task_id}`、`DELETE /v1/tasks/{task_id}`）
- **模型列表** —— `GET /v1/models` 返回所有可用工作流（模型）
- **后端状态查询** —— `GET /v1/backends` 查看各后端健康状态
- **健康检查** —— `GET /v1/health` 存活探针
- **视频子系统状态** —— `GET /v1/videos/health`
- **Prometheus 指标** —— `GET /v1/metrics`
- **API 帮助文档** —— `GET /v1/help`

### 多后端管理
- 支持配置多个 ComfyUI 后端节点，按名称 (`?backend=xxx`) 显式指定或自动选择
- **定期健康检查**：通过请求每个后端的 `/system_stats`，连续失败达阈值自动摘除，恢复后自动加入
- **负载均衡策略**：轮询（Round Robin）、最少连接数（Least Connections）、随机（Random），可在配置文件中切换

### 智能工作流注入
代理自动解析工作流 JSON，根据节点 `class_type` 和 `_meta.title` 定位并注入参数：

| 节点类型 | 注入参数 |
|---------|---------|
| `CLIPTextEncode` (Positive) | 正向提示词 `text` |
| `CLIPTextEncode` (Negative) | 负向提示词 `text` |
| `EmptyLatentImage` 等 | `width`、`height`、`batch_size` |
| `LoadImage` | 参考图文件名（自动上传至 ComfyUI） |
| `RandomNoise` / `KSampler` | `noise_seed` / `seed` |
| `PrimitiveInt` / `PrimitiveFloat` / `FloatSlider` | 时长、帧率、尺寸等数值 |
| `PromptRelayEncode` | 多镜头提示词与分段长度 |
| `LTXVAddGuideMulti` | 多图引导帧索引 |

### 参考图处理
- 支持 Base64 内联图片和 HTTP URL 两种输入方式
- 自动将图片上传至 ComfyUI 的 `/upload/image` 端点并注入到 `LoadImage` 节点
- 无参考图时自动使用 1×1 透明占位图，避免工作流中断
- 内存缓存 + 本地文件系统双重加速（超过 1000 条自动清理一半旧条目）

### 生产可靠性
- **指数退避 + 完全抖动**：ComfyUI 历史轮询采用全抖动算法，避免惊群效应
- **WebSocket 双通道**（可选）：连接默认后端获取实时任务完成通知，网络中断时自动降级为 HTTP 轮询
- **令牌桶限流**：可配置速率限制，过载时返回 `429 Too Many Requests`
- **幂等键支持**：通过 `Idempotency-Key` 请求头防止重复提交，缓存 TTL 24 小时
- **请求级响应缓存**：相同参数组合（模型 + 提示词哈希 + 尺寸 + 种子）命中时直接返回缓存结果
- **优雅关闭**：收到 SIGTERM 后停止接收新请求，等待进行中任务完成或超时后退出
- **任务持久化**：所有任务状态存储至 `tasks.json`，服务重启后自动恢复

### 可观测性
- **Prometheus 指标** `/v1/metrics`：`total_requests`、`active_tasks`、`request_duration_seconds`、`cache_hit_total`
- **OpenTelemetry 分布式追踪**（可选）：通过 OTLP 导出至 Jaeger/Tempo，为每个请求分配 Trace ID
- **结构化日志**：Base64 图像数据自动替换为 `[base64 omitted]`，长载荷截断处理
- **API 文档** `/v1/help`：返回所有接口的详细 JSON 文档

### 种子稳定性追踪
- 通过 `X-Consistent-Role` 请求头指定角色名称，代理自动追踪并复用该角色上次成功的种子值
- 适用于需要保持角色外貌一致性的多分镜生成场景（如 LocalMiniDrama 的角色一致性需求）

## 快速开始

### 前置条件
- Rust 1.70+
- ComfyUI 后端（需启用 `--api` 模式）
- （可选）Docker 及 Docker Compose

### 安装步骤

**1. 克隆仓库**
```bash
git clone https://github.com/553556705-tech/comfyui-openai-api.git
cd comfyui-openai-api/apps/rust/comfyui-openai-api
```

**2. 创建配置文件**
```bash
cp config/config.sample.yaml config/config.yaml
```

**3. 编辑 `config/config.yaml`**

```yaml
log_level: "info"
server:
  host: "0.0.0.0"
  port: 8080

# 多后端列表
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
  use_ws: true               # 设置为 false 可关闭 WebSocket
  input_dir: "./cache"       # 代理本地图片缓存目录

routing:
  timeout_seconds: 3600
  max_payload_size_mb: 500
  Image_Width: 1280
  Image_Height: 704
  video_Width: 1024
  video_Height: 576
  fps: 24
  free_model_before_video: true

  # 负载均衡策略 (RoundRobin / LeastConnections / Random)
  lb_strategy: "RoundRobin"

  # 令牌桶限流（可选，注释则关闭）
  rate_limit:
    max_tokens: 60
    refill_rate: 1.0

  # 请求级响应缓存（可选，注释则关闭）
  response_cache:
    ttl_secs: 600
    max_entries: 500

  # 幂等键支持
  enable_idempotency: true

  # 优雅关闭等待秒数
  graceful_shutdown_timeout_secs: 30

  # 后端健康检查参数
  health_check_interval_secs: 15
  health_check_fail_threshold: 3
```

**4. 准备 ComfyUI 工作流文件**

将工作流 JSON 文件放入 `workflows/` 目录，确保节点标题包含可识别的关键字（如 "Positive"、"Negative"、"Width"、"Height"、"Reference Image" 等）。

### 运行

**本地编译运行**
```bash
cargo build --release
./target/release/comfyui-openai-api
```

**Docker 方式**
```bash
docker build -t comfyui-openai-api .
docker run -p 8080:8080 \
  -v $(pwd)/config:/app/config \
  -v $(pwd)/workflows:/app/workflows \
  comfyui-openai-api
```

## API 端点详解

所有端点均以 `/v1` 为前缀。以下是完整列表：

| 端点 | 方法 | 说明 |
|------|------|------|
| `/v1/models` | GET | 列出所有可用模型（工作流文件名） |
| `/v1/health` | GET | 简单存活检查 |
| `/v1/backends` | GET | 查看所有后端健康状态 |
| `/v1/images/generations` | POST | 图像生成，同步返回 |
| `/v1/videos/generations` | POST | 视频生成，异步返回 `task_id` |
| `/v1/tasks` | GET | 列出所有任务状态 |
| `/v1/tasks/{task_id}` | GET / DELETE | 查询或删除单个任务 |
| `/v1/videos/health` | GET | 视频生成子系统状态 |
| `/v1/metrics` | GET | Prometheus 指标导出 |
| `/v1/help` | GET | API 帮助文档（JSON） |

### 1. 图像生成 `POST /v1/images/generations`

**请求示例**
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

**请求参数（Body）**

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `model` | string | 是 | 工作流文件名（不含 `.json`） |
| `prompt` | string | 否 | 正向提示词 |
| `negative_prompt` | string | 否 | 负向提示词 |
| `size` | string | 否 | 尺寸，如 `"1024x1024"`（可被配置文件覆盖） |
| `seed` | integer | 否 | 随机种子 |
| `n` | integer | 否 | 生成数量（批次大小） |
| `reference_images` | array | 否 | 参考图数组 `[{name, data}]` |
| `image` | array | 否 | Base64 图片字符串数组（等效于 `reference_images`） |

**查询参数**

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `backend` | string | 否 | 指定后端名称，未指定时自动使用负载均衡策略 |

**响应格式**
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

### 2. 视频生成 `POST /v1/videos/generations`

**请求示例**
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

**请求参数（Body）**

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `model` | string | 是 | 视频工作流文件名 |
| `content` | array | 是 | 内容数组，每项可为 `{"type":"text", "text":"..."}` 或 `{"type":"image_url", "image_url":{"url":"..."}, "role":"reference_image"}` |
| `duration` | integer | 否 | 时长（秒），默认 5 |
| `resolution` | string | 否 | `"720p"` 或 `"1080p"` |
| `ratio` | string | 否 | 宽高比，如 `"16:9"` |
| `local_prompts` | string | 否 | 多镜头提示词，格式 `[起始-结束]\n描述` |
| `global_prompt` | string | 否 | 全局提示词 |
| `guide_strengths` | array | 否 | 引导强度数组 |

**响应格式**
```json
{
  "task_id": "vid-1715432100123-789"
}
```

### 3. 任务查询 `GET /v1/tasks/{task_id}`

```bash
curl http://localhost:8080/v1/tasks/vid-1715432100123-789
```

响应示例（处理中）：
```json
{
  "status": "processing"
}
```

响应示例（已完成）：
```json
{
  "status": "completed",
  "video_url": "http://backend-b:8000/view?filename=video.mp4&subfolder=&type=output",
  "b64_json": "..."
}
```

响应示例（失败）：
```json
{
  "status": "failed",
  "error": "ComfyUI node error: ..."
}
```

### 4. 列出所有任务 `GET /v1/tasks`

```bash
curl http://localhost:8080/v1/tasks
```

返回：
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

### 5. 删除任务 `DELETE /v1/tasks/{task_id}`

```bash
curl -X DELETE http://localhost:8080/v1/tasks/img-1715432100456-123
```
成功时返回 HTTP `204 No Content`。

### 6. 其他端点

- **列出模型** `GET /v1/models`
```bash
curl http://localhost:8080/v1/models
```
响应：
```json
{
  "object": "list",
  "data": [
    { "id": "sdxl-workflow", "object": "model", "owned_by": "comfyui-openai-api" },
    { "id": "video-workflow", "object": "model", "owned_by": "comfyui-openai-api" }
  ]
}
```

- **后端健康状态** `GET /v1/backends`
```json
{
  "backends": [
    { "name": "backend-a", "healthy": true },
    { "name": "backend-b", "healthy": false }
  ]
}
```

- **存活探针** `GET /v1/health`：返回 `OK`

- **Prometheus 指标** `GET /v1/metrics`：返回标准 Prometheus 文本格式指标。

### Python 客户端集成

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

## 工作流管理

### 自动注入原理

代理在收到请求后会加载对应的工作流 JSON 文件，并遍历所有节点，基于节点类型和标题识别注入点：

- **CLIPTextEncode**：标题中含有 "Positive" 的节点注入正向提示词，含有 "Negative" 的注入负向提示词。
- **EmptyLatentImage / EmptySD3LatentImage / EmptyFlux2LatentImage**：注入 `width`、`height`、`batch_size`。
- **LoadImage**：按标题中的 "Reference Image 1"、"Reference Image 2" 等序号排序后，依次注入参考图的文件名（参考图已被代理上传至 ComfyUI）。
- **PrimitiveInt / INTConstant / PrimitiveFloat / FloatSlider**：标题为 "Width"、"Height"、"Duration"、"FPS" 等会被注入相应的数值。
- **RandomNoise / KSampler**：统一注入生成用的随机种子。
- **PromptRelayEncode**：自动构建 `local_prompts` 和 `segment_lengths`（用于多镜头视频）。
- **LTXVAddGuideMulti**：根据参考图数量动态计算并注入引导帧索引。

### 创建自定义工作流

1. 在 ComfyUI 中设计工作流，并为关键节点设置具有语义的**标题（title）**：
   - 正向提示词 CLIPTextEncode → 标题包含 **"Positive"**
   - 负向提示词 CLIPTextEncode → 标题包含 **"Negative"**
   - 宽度整数/浮点节点 → **"Width"**
   - 高度整数/浮点节点 → **"Height"**
   - 帧率节点 → **"FPS"**
   - 时长节点 → **"Duration"**
   - 参考图 LoadImage 节点 → **"Reference Image 1"**、**"Reference Image 2"** …（序号将决定注入顺序）
2. 保存工作流为 JSON 文件（文件名将作为 `model` 参数的值）。
3. 将 JSON 文件放入配置的 `workflows_folder` 目录。
4. 启动代理，即可通过指定 `model` 参数调用该工作流。

## 配置参考

### 环境变量

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `CONFIG_PATH` | 配置文件路径 | `./config/config.yaml` |
| `RUST_LOG` | 日志级别（可被配置文件覆盖） | `info` |

### 完整配置项详解

```yaml
# 日志级别：trace, debug, info, warn, error
log_level: "info"

# 代理服务绑定地址和端口
server:
  host: "0.0.0.0"
  port: 8080

# 多后端列表，至少需要一个后端
comfyui_backends:
  - name: "backend-a"        # 唯一名称，用于通过 ?backend= 选择
    host: "127.0.0.1"        # ComfyUI 地址
    port: 8000               # ComfyUI 端口
    default: true            # 是否默认后端（用于 WebSocket 连接）
  - name: "backend-b"
    host: "192.168.1.100"
    port: 8188
    default: false

# 代理内部设置
comfyui_backend:
  client_id: "comfyui-api"         # WebSocket 客户端 ID
  workflows_folder: "./workflows"  # 工作流 JSON 存放目录
  use_ws: true                     # 是否启用 WebSocket 连接默认后端
  input_dir: "./cache"             # 图片缓存目录，保存上传的参考图

# 路由与运行时配置
routing:
  timeout_seconds: 3600            # ComfyUI 任务总超时（秒）
  max_payload_size_mb: 500         # 请求体最大大小（MB）
  Image_Width: 1280                # 图像默认宽度（可被请求中的 size 覆盖）
  Image_Height: 704                # 图像默认高度
  video_Width: 1024                # 视频默认宽度
  video_Height: 576                # 视频默认高度
  fps: 24                          # 默认帧率
  free_model_before_video: true    # 生成视频前是否调用 ComfyUI /free 释放显存

  # 负载均衡策略，可选值：RoundRobin, LeastConnections, Random
  lb_strategy: "RoundRobin"

  # 令牌桶限流（可选，注释或删除则限流不生效）
  rate_limit:
    max_tokens: 60       # 桶容量
    refill_rate: 1.0     # 每秒补充令牌数

  # 请求级响应缓存（可选，注释或删除则缓存不生效）
  response_cache:
    ttl_secs: 600        # 缓存有效期（秒）
    max_entries: 500     # LRU 最大条目数

  # 是否启用幂等键检查（Idempotency-Key 头）
  enable_idempotency: true

  # 优雅关闭最长等待时间（秒），超时后强制退出
  graceful_shutdown_timeout_secs: 30

  # 健康检查间隔（秒）与连续失败阈值
  health_check_interval_secs: 15
  health_check_fail_threshold: 3
```

## 项目结构

```
apps/rust/comfyui-openai-api/
├── Cargo.toml
├── build.sh
├── Dockerfile
├── tasks.json                 # 任务持久化文件（自动生成，可 .gitignore）
├── config/
│   ├── config.yaml            # 运行配置（不提交至仓库）
│   └── config_sample.yaml     # 配置模板
├── workflows/                 # 示例工作流（建议 .gitignore 整个目录或具体文件）
│   └── *.json
└── src/
    ├── main.rs                # 服务入口，路由注册与中间件
    ├── config.rs              # 配置结构定义与 YAML 解析
    ├── proxy.rs               # 全局共享状态 ProxyState
    ├── error.rs               # 统一错误处理（OpenAI 风格错误格式）
    ├── seed_tracker.rs        # 角色种子稳定性追踪
    ├── graceful.rs            # 优雅关闭（等待任务排水）
    ├── tracing_setup.rs       # OpenTelemetry 初始化
    ├── task_manager.rs        # 任务持久化、增删查改
    ├── workflows/
    │   ├── mod.rs
    │   ├── registry.rs        # 工作流注册表，支持模型列表
    │   └── template.rs        # 预解析注入点映射（PreparedWorkflow）
    ├── handlers/
    │   ├── mod.rs
    │   ├── image.rs           # /v1/images/generations 图像生成
    │   ├── video.rs           # /v1/videos/generations 视频生成
    │   ├── tasks.rs           # /v1/tasks 系列
    │   ├── metrics.rs         # /v1/metrics Prometheus 指标
    │   ├── models.rs          # /v1/models 模型列表
    │   ├── health.rs          # /v1/health 健康检查
    │   └── backends.rs        # /v1/backends 后端状态
    ├── backend/
    │   ├── mod.rs
    │   ├── pool.rs            # 后端连接池、健康检查、负载均衡
    │   └── router.rs          # 路由策略占位（可扩展）
    ├── transport/
    │   ├── mod.rs
    │   ├── poll.rs            # HTTP 历史轮询（全抖动退避）
    │   └── ws.rs              # WebSocket 实时通知管理
    ├── middleware/
    │   ├── mod.rs
    │   ├── rate_limiter.rs    # 令牌桶限流器
    │   ├── request_id.rs      # 请求 ID 中间件
    │   └── idempotency.rs     # 幂等键存储与检查
    └── cache/
        ├── mod.rs
        ├── image_cache.rs     # 图片本地缓存与自动上传
        └── response_cache.rs  # LRU 请求级响应缓存
```

## 架构与请求流程

### 架构概览

```
┌──────────────────────────────────────┐
│         OpenAI 兼容客户端            │
│   (Python, JS, curl, LocalMiniDrama) │
└────────────────┬─────────────────────┘
                 │ HTTP POST /v1/images/generations
                 ▼
┌──────────────────────────────────────┐
│        comfyui-openai-api            │
│  ┌──────────┐ ┌────────────────┐     │
│  │ 限流器   │ │ 幂等键检查     │     │
│  └──────────┘ └────────────────┘     │
│  ┌──────────┐ ┌────────────────┐     │
│  │ 工作流   │ │ 后端池 & LB    │     │
│  │ 注入器   │ │ + 健康检查     │     │
│  └──────────┘ └────────────────┘     │
│  ┌──────────────────────────────┐    │
│  │ WebSocket / HTTP 轮询        │    │
│  │ (全抖动退避)                 │    │
│  └──────────────────────────────┘    │
└────────────────┬─────────────────────┘
                 │
                 ▼
┌──────────────────────────────────────┐
│  ComfyUI 后端 A (健康)               │
│  ComfyUI 后端 B (健康)               │
│  ComfyUI 后端 C (不健康，已摘除)     │
└──────────────────────────────────────┘
```

### 单次图像请求完整链路

```
客户端 POST /v1/images/generations?backend=xxx
    │
    ▼
[1] 优雅关闭检查                → 若关闭中，返回 500
[2] 幂等键检查（可选）          → 命中则直接返回缓存响应
[3] 令牌桶限流（可选）          → 超限则返回 429
[4] 后端选择                    → 显式指定或负载均衡策略
[5] 工作流模板加载              → 从 WorkflowRegistry 获取 PreparedWorkflow
[6] 请求转换与参数注入          → 种子、提示词、尺寸、参考图等
[7] 提交至 ComfyUI             → POST /prompt，获得 prompt_id
[8] 等待完成                    → 优先通过 WebSocket 监听，降级为 HTTP 轮询（全抖动退避）
[9] 图片下载                    → /view 下载 → Base64 编码
[10] OpenAI 格式响应           → {"created":..., "data":[{"b64_json":"..."}]}
[11] 缓存写入（可选）           → 请求级响应缓存 + 幂等键缓存
[12] 种子追踪更新（可选）       → 更新对应角色的成功种子值
```

## 性能特性

| 特性 | 说明 |
|------|------|
| 异步 I/O | 基于 Tokio 运行时，非阻塞处理并发请求 |
| 连接复用 | Reqwest HTTP 客户端启用 TCP_NODELAY 与连接池 |
| 轮询优化 | 指数退避 + 完全抖动，避免惊群效应 |
| 内存控制 | 图片缓存超过 1000 条自动清理一半；LRU 响应缓存可配置上限 |
| 任务持久化 | HashMap + `tasks.json`，重启不丢状态 |
| 可配置超时 | 全局请求超时、轮询超时、优雅关闭超时均可独立设置 |
| 请求大小限制 | `max_payload_size_mb` 防止巨型请求 |

## 监控与调试

### 日志控制

通过配置文件中的 `log_level` 或环境变量 `RUST_LOG` 控制日志级别：

| 级别 | 输出内容 |
|------|---------|
| `trace` | 完整请求/响应体、工作流 JSON 细节 |
| `debug` | 关键路径日志、WebSocket 状态 |
| `info` | 请求入口、后端选择、任务完成（默认） |
| `warn` | 重试、降级、可恢复错误 |
| `error` | 连接失败、任务异常 |

### Prometheus 指标 (`/v1/metrics`)

| 指标名 | 类型 | 描述 |
|--------|------|------|
| `total_requests` | Counter | 总请求数 |
| `active_tasks` | Gauge | 当前活跃任务数 |
| `request_duration_seconds` | Histogram | 请求端到端延迟分布 |
| `cache_hit_total` | Counter | 响应缓存命中次数 |

### 分布式追踪

启用 OpenTelemetry 后，代理通过 OTLP 协议向 Jaeger/Tempo 等后端导出 Trace。每个请求分配全局唯一 `x-request-id`，关键处理阶段均包装为 Span。

### 内置任务面板

每 5 秒在日志中输出当前所有任务的状态摘要：

```
📋 Task status (3 total):
   🟡 img-1715432100123-456 - Processing
   🟢 vid-1715432100456-789 - Completed
   🔴 img-1715432100789-012 - Failed: ComfyUI error: ...
```

## 与 LocalMiniDrama 的生态协同

- **统一生成接口**：`LocalMiniDrama` 通过标准 OpenAI API 调用本代理，无需关心底层 ComfyUI 工作流细节。
- **批量分镜生成**：逐镜生成流程可借助多后端负载均衡实现并行加速。
- **角色一致性**：通过 `X-Consistent-Role` 头与种子追踪器联动，保持同一角色多分镜外貌一致。
- **视频模型支持**：内置豆包 Seedance、通义万相、Vidu 等工作流注入兼容，覆盖短剧制作的多模型需求。

## 故障排查

| 现象 | 可能原因 | 排查方法 |
|------|---------|---------|
| 502 Bad Gateway | ComfyUI 后端不可达 | 检查 `comfyui_backends` 配置中 host/port 是否正确，确认 ComfyUI 已启动 `--api` |
| 404 Workflow not found | `model` 参数对应的工作流文件不存在 | 确认 `workflows_folder` 目录下存在 `{model}.json` 文件 |
| 400 Invalid request | 请求体格式错误或 Base64 解码失败 | 检查 JSON 格式，验证 Base64 编码有效性 |
| 504 Timeout | 生成时间超过 `timeout_seconds` | 增大超时值或检查 ComfyUI 日志中的节点错误信息 |
| 429 Too Many Requests | 触发令牌桶限流 | 调整 `rate_limit` 配置或降低请求频率 |
| 后端被摘除 | 健康检查连续失败 | 检查 ComfyUI `/system_stats` 是否正常返回，网络连通性 |

## 版本历史

### v0.3.0
- 🏗️ 模块化架构重构，拆分 handlers/backend/transport/middleware/cache/workflows
- 🔄 多后端健康检查与负载均衡（RoundRobin / LeastConnections / Random）
- 🔒 令牌桶限流中间件
- 🆔 幂等键支持
- 💾 请求级 LRU 响应缓存
- 📡 OpenTelemetry 分布式追踪
- 🧹 优雅关闭与任务排水
- 🌱 角色种子稳定性追踪器
- 📋 新增 `/v1/models`、`/v1/backends`、`/v1/tasks` 列表与删除等端点

### v0.2.0
- 视频生成支持
- 多后端手动路由
- 任务持久化（tasks.json）
- PromptRelayEncode / LTXVAddGuideMulti 节点注入

### v0.1.0
- 初始版本，OpenAI 图像生成兼容

## 贡献指南

欢迎通过 Issue 和 Pull Request 参与贡献。请遵循以下准则：

- 为新增的公共函数和模块添加文档注释
- 面向用户的功能改动需同步更新配置示例和 API 文档（即本 README 与 /v1/help 端点）
- 针对多种工作流配置进行测试
- 遵循现有代码风格和模块组织方式

## 许可证

MIT License

---

*此项目为 [LocalMiniDrama](https://github.com/553556705-tech/LocalMiniDrama.git) 生态系统的一部分，为本地 AI 短剧创作提供稳定、可扩展的底层生成引擎 API 基座。*
```