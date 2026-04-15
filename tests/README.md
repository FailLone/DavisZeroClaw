# Tests

仓库内的测试资产统一收口到这个目录：

- `rust/`：Rust 单元测试与测试辅助代码，通过 `src/lib.rs` 的 `#[path = "../tests/rust/mod.rs"]` 接入 `cargo test`
- `scripts/`：需要连真实运行环境的 shell 冒烟 / 端到端测试
- `fixtures/`：预留给测试夹具与样例数据

当前推荐入口：

```bash
cargo test
```

```bash
./tests/scripts/test_ha_real.sh
```

```bash
./tests/scripts/test_ha_webhook_e2e.sh
```
