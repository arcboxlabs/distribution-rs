# Phase 5：配置、通知与运维能力

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**目标：** 实现完整 YAML 配置加载、webhook 通知、健康检查、Prometheus metrics、结构化日志。达到可生产部署状态。

**包含模块：** `registry-config`、`registry-notifications`、`registry-health`，以及 `registry-http` 中的 health/metrics handler 和 tracing 集成。

## 关键决策点

### 1. 配置加载（registry-config）

`figment 0.10.x`（YAML 文件 + 环境变量覆盖）。`Configuration` struct 精确对应 Go 的 `configuration/configuration.go` 顶层字段：

| 字段 | 说明 |
|------|------|
| `version` | 配置版本 |
| `log` | 日志配置 |
| `storage` | 存储 driver 配置 |
| `auth` | 认证配置 |
| `http` | HTTP 服务配置 |
| `notifications` | Webhook 通知配置 |
| `health` | 健康检查配置 |
| `redis` | Redis 配置（descriptor 缓存） |
| `catalog` | catalog API 配置 |
| `proxy` | pull-through cache 配置 |
| `validation` | 镜像校验配置 |

storage driver 参数用 `HashMap<String, serde_json::Value>` 对应 Go 的 `Parameters`。支持 `${ENV_VAR}` 替换（反序列化前预处理）。

**选 figment 而非 `config` crate 原因**：figment 反序列化错误信息更具体（精确到字段名），对配置调试体验更好。

### 2. Notifications（registry-notifications）

```rust
let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<NotificationEnvelope>();
tokio::spawn(async move {
    while let Some(env) = rx.recv().await {
        for sink in &sinks { sink.send(env.clone()).await; }
    }
});
```

- HTTP endpoint sink 用 `reqwest 0.12.x` POST，指数退避重试（`tokio::time::sleep` 实现，不引入额外 backoff crate）
- Event 格式与 Go 完全兼容（`application/vnd.docker.distribution.events.v2+json`）

### 3. Prometheus metrics

使用 `prometheus 0.13.x`：

| metric | 类型 | 标签 |
|--------|------|------|
| `registry_http_requests_total` | counter | method, route, status |
| `registry_storage_action_duration_seconds` | histogram | driver, action |
| `registry_blob_size_bytes` | histogram | - |

暴露 `GET /metrics`（Prometheus text format）

### 4. 健康检查（registry-health）

- `GET /healthz` 返回 JSON `{"status":"ok"}`
- Filesystem driver 健康检查：验证 root 目录可写
- S3 driver：执行一次 list 操作

### 5. 结构化日志

`tracing 0.1.x` + `tracing-subscriber` JSON format。每个请求附加字段：

| 字段 | 说明 |
|------|------|
| `request_id` | UUID，对应 Go 的 `dcontext.GetLogger(ctx)` |
| `repository` | 仓库名 |
| `method` | HTTP 方法 |
| `path` | 请求路径 |

## 依赖 crate

| crate | 版本 | 用途 |
|-------|------|------|
| `figment` | 0.10.x | YAML 配置 + 环境变量覆盖 |
| `serde_yaml` | 0.9.x | YAML 反序列化 |
| `reqwest` | 0.12.x | Webhook HTTP sink |
| `prometheus` | 0.13.x | metrics 采集与暴露 |
| `tracing` | 0.1.x | 结构化日志 |
| `tracing-subscriber` | - | JSON log format |
| `opentelemetry` | 0.27.x | 分布式追踪 |
| `redis` | 0.27.x | descriptor 缓存（保守选择，优先于 fred） |

## 完成标准

```bash
# 配置校验
cargo run -p registry-bin -- --config /path/to/config.yaml --check-config
# → Config OK（或具体错误位置）

# 健康检查
curl http://localhost:5000/healthz
# → {"status":"ok"}

# Metrics 端点
curl http://localhost:5000/metrics | grep registry_http_requests_total
# → 有输出

# Notifications（配置 webhook endpoint）
docker push localhost:5000/test/image:latest
# → webhook 在 5 秒内收到 action="push" Event，包含 target.repository/digest/tag

# Phase 3/4 regression
OCI_ROOT_URL=http://localhost:5000 ./conformance.test -test.v  # 仍全绿
docker push/pull 仍然正常工作
```

## 准备工作

```bash
# Webhook 接收端测试
nc -l 8080
# 或使用 webhook.site
```
