# DavisZeroClaw MVP Todo

这份清单只记录当前 MVP 的实际进度，避免在精简文档后丢失上下文。

## 已完成

- [x] 用官方 `channels_config.webhook` 取代 no-tools 的 `POST /webhook`
- [x] 确认 `Shortcut -> Webhook Channel -> HA MCP` 主链路可工作
- [x] 将启动方式固定为 `zeroclaw gateway start` + `zeroclaw channel start`
- [x] 重写 README，使其面向用户逐步上手

## 当前保留的体验边界

- [x] Shortcut 的 `200 OK` 只表示 ZeroClaw 已接单
- [x] 正式的完成回执方案尚未确定
- [x] 不把 Siri / HomePod 异步播报当成 MVP 成功条件

## 下一步

- [ ] 用当前 `.env.local` 完整启动一次新版脚本
- [ ] 产出一个可直接导入的 Shortcut 模板
- [ ] 视需要决定是否启用 `channels_config.webhook.secret`
- [ ] 研究任务完成后的语音播报 / 回执方案
