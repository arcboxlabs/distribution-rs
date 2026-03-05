# Phase 4：Auth 与中间件

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**目标：** 实现 `AccessController` trait + token JWT auth + htpasswd auth，使 `docker login / push / pull` 端到端可用。

**包含模块：** `registry-auth`（token/、htpasswd/、silly/），`registry-http/src/middleware/auth.rs`

## AccessController trait 定义

```rust
#[async_trait]
pub trait AccessController: Send + Sync {
    async fn authorized(
        &self,
        headers: &HeaderMap,
        access: &[Access],
    ) -> Result<Grant, AuthError>;
}

pub enum AuthError {
    Challenge(AuthChallenge),  // → 401 + WWW-Authenticate
    Unauthorized(String),
    Internal(anyhow::Error),
}
```

`AuthChallenge` 携带 realm/service/scope，由 HTTP 中间件写入 `WWW-Authenticate` 响应头。解耦了 Go 中 `Challenge` interface 同时是 error 又写 HTTP header 的问题。

**Go 原始接口** (`registry/auth/auth.go`)：
```go
type AccessController interface {
    Authorized(r *http.Request, access ...Access) (*Grant, error)
}
```

## 关键决策点

1. **AccessController trait**：见上方定义。`AuthChallenge` 携带 realm/service/scope，由 HTTP 中间件写入 `WWW-Authenticate` 响应头。解耦了 Go 中 `Challenge` interface 同时是 error 又写 HTTP header 的问题。

2. **Token auth 流程**（移植 `registry/auth/token/accesscontroller.go`）：
   - 从 `Authorization: Bearer <token>` 提取 JWT
   - 用 `jsonwebtoken 9.x` 验证 RS256/ES256 签名（从 PEM 或 JWKS 加载公钥）
   - 验证 issuer/audience/expiry
   - 对比 token `access` claims 与请求所需 access
   - 失败返回 `AuthError::Challenge`（携带 `WWW-Authenticate: Bearer realm=...,service=...,scope=...`）

3. **htpasswd**：仅支持 bcrypt（`$2y$`，使用 `bcrypt 0.15.x`）。SHA1/MD5 格式写警告日志并拒绝（不安全）。文件 mtime 变化时动态重载。

4. **auth 中间件位置**：中间件只处理"无 token"场景（返回 challenge）。细粒度 scope 检查在各 handler 内调用 `access_controller.authorized()`，与 Go 的 `app.authorized()` 位置一致。

5. **无 auth 模式**：config 无 `auth:` 节时注入 `NullAccessController`，直接返回全权 `Grant`。

## 依赖 crate

| crate | 版本 | 用途 |
|-------|------|------|
| `jsonwebtoken` | 9.x | JWT RS256/ES256 验证 |
| `bcrypt` | 0.15.x | htpasswd bcrypt 密码验证 |

## 注意事项

- **token auth JWT 细节兼容性**（Docker token service 的 `access` claim 格式、算法协商）：从 `registry/auth/token/token_test.go` 提取测试向量，在 Rust 单元测试中用相同数据验证
- **axum 中间件无法访问 path 参数**：scope 检查移入各 handler 函数（与 Go 的 `app.authorized()` 位置一致），中间件只负责无 token 时返回 challenge

## 完成标准

```bash
# 配置 token auth（测试 RSA key pair）
docker login localhost:5000 -u testuser -p testpassword
# → Login Succeeded

docker pull ubuntu:22.04
docker tag ubuntu:22.04 localhost:5000/myorg/ubuntu:22.04
docker push localhost:5000/myorg/ubuntu:22.04
docker pull localhost:5000/myorg/ubuntu:22.04
# → 全部成功

# 未认证请求返回 401
curl -sv http://localhost:5000/v2/myorg/ubuntu/tags/list 2>&1 | grep -E "401|WWW-Authenticate"
# → HTTP/1.1 401 Unauthorized
# → WWW-Authenticate: Bearer realm=...
```

## 准备工作

```bash
# 测试用 RSA key pair
openssl genrsa -out private.pem 2048 && openssl rsa -in private.pem -pubout -out public.pem
```
