# 项目概览

OCI Distribution Registry 的 Rust 重写，目标通过 OCI Distribution Spec conformance test。Go 参考实现：`/Users/zhangbin/Arcbox/distribution/`，规划文档：`docs/overview.md`。

**技术栈：** tokio 1.x · axum 0.8.x · async-trait 0.1.x · serde 1.x · thiserror 2.x · sha2/hmac 0.10/0.12.x · figment 0.10.x · object_store 0.11.x · jsonwebtoken 9.x · reqwest 0.12.x · tracing · prometheus 0.13.x · clap 4.x

# 行为规定
1. **直接写代码，不要输出计划** — 收到实现任务时，立即读代码、写代码。不要先输出"我将执行以下步骤"的计划文本。如果需要规划，使用 plan mode，不要在聊天中罗列步骤。
2. **对话式协作，而非自主探索** — 每完成一个文件的修改后，简要说明改了什么、为什么这样改，然后继续。不要一次性沉默地改 10 个文件后才汇报。遇到设计抉择时主动问我。
3. **编辑后验证引用完整性** — 修改函数签名、重命名类型/模块、删除 pub 项时，必须 grep 搜索所有引用点并同步更新，确保 `cargo check` 通过。
4. **默认 Rust 惯用模式** — 优先使用 `Result<T, E>` 而非 panic、`impl Trait` 而非 dyn、`thiserror` 派生错误类型、Builder pattern 构建复杂结构体。不需要我每次指定。
5. **逐文件解释，不要高层概述** — 解释代码变更时，按文件逐个说明具体改动（哪些行、什么逻辑），而不是"我重构了错误处理系统"这样的抽象描述。
6. **改完代码必须跑测试** — 任何代码修改完成后，必须运行 `cargo test` 并报告结果。测试失败时先修复再继续，不要跳过。