# DavisZeroClaw MVP Todo

这份清单只保留 V1 MVP 仍需要推进的内容，并按优先级排序。

## MVP 范围

- macOS 本地脚本化安装可稳定启动并持续运行
- `Siri / Shortcut -> Webhook Channel -> ZeroClaw -> Home Assistant` 主链路可复用
- 支持 1-2 家可用模型接入
- 支持基础家居控制、基础自然语言问答、只读调查和快递查询

## 当前状态

截至 `2026-03-24`，以下内容已完成：

- `zeroclaw gateway start` + `zeroclaw channel start` 启动链路已验证
- `.env.local`、运行时配置模板、启动脚本、停止脚本已补齐
- `README.md` 与 Shortcut 配置文档已重写，可支撑脚本化安装
- Home Assistant MCP 已接入，基础主链路可跑通
- 只读调查能力已落地为 `ha-audit` skill + `http_request` + 本地只读 `ha_audit_proxy`
- `DAVIS_HA_TOKEN` 已确认可读取 HA REST `config/logbook/history`
- `recorder/history/logbook` 已在真实 HA 环境中验证有足够数据
- 调查类请求已支持实体解析、只读约束和时间线返回

当前已知缺口：

- 历史语义问题不一定稳定触发 `ha-audit`，仍可能误走 `GetLiveContext`
- 读取类回复里仍有亮度百分比表述异常，例如 `130%`

## P0

这些项直接影响 MVP 是否可交付，优先完成。

- [ ] 产出一个可直接导入的 Shortcut 模板
- [ ] 将模板动作固定为“语音/文本输入 -> JSON 请求 -> 接单确认朗读”
- [ ] 明确 Shortcut 后续动作边界：MVP 至少能接收结构化结果并继续执行朗读或跳转
- [ ] 在真实 HA 环境中验证灯光控制
- [ ] 在真实 HA 环境中验证开关控制
- [ ] 整理一组标准验收口令，覆盖房间名、设备名、开/关两类常见表达
- [ ] 验证基础自然语言问答可用，确保非控制类请求不会误触发设备操作
- [ ] 强化历史类问题的路由，确保“昨晚/之前/某个时间段/谁关的”这类问题优先进入 `ha-audit`
- [ ] 为查询类结果定义适合 Shortcut / Siri 播报的简短返回格式
- [ ] 修正读取类回复中的亮度百分比换算或表述问题；`2026-03-23` 的书房灯带只读测试返回了 `亮度为 130%`
- [ ] 补充常见故障排查说明：端口占用、HA Token 错误、模型 Key 缺失、MCP 连不上

## P1

这些项对 V1 完整性重要，但可以排在 P0 之后。

- [ ] 至少验证 1 家中国大陆可用的 OpenAI 格式模型提供方
- [ ] 视情况补充第 2 家备选模型提供方，满足 PRD 的“支持配置 1-2 家模型 API”
- [ ] 明确默认 provider、可替换 provider 和所需环境变量
- [ ] 验证模型切换后，Tool 调用链和 Home Assistant 控制仍然稳定
- [ ] 视部署场景决定是否启用 `channels_config.webhook.secret`
- [ ] 如果启用 `secret`，补齐 Shortcut 侧签名生成说明
- [ ] 明确 Home Assistant MCP 不可用时的处理策略，决定是否保留 `ha_tool` 备份方案
- [ ] 实现并接入快递查询工具，这是 PRD 已写入的 MVP 范围

## P2

这些项有价值，但不阻塞当前 V1 验收。

- [ ] 将当前 Python 版 `ha_audit_proxy` 重写为 Rust 实现，减少运行时依赖，并与 ZeroClaw 的 Rust 技术栈保持一致
- [ ] 正式的“任务完成回执”方案延后，当前只提供接单确认

## 已完成里程碑

- [x] 用官方 `channels_config.webhook` 取代 no-tools 的 `POST /webhook`
- [x] 确认 `Shortcut -> Webhook Channel -> Home Assistant MCP` 主链路可工作
- [x] 将启动方式固定为 `zeroclaw gateway start` + `zeroclaw channel start`
- [x] 提供 `.env.example`、运行时配置模板、启动脚本和停止脚本
- [x] 在配置模板中接入 `memory.sqlite` 与 `homeassistant` MCP Server
- [x] 重写 `README.md`，使其面向用户逐步上手
- [x] 补充 `docs/Shortcut_and_Companion_Setup.md`，明确 Shortcut 请求格式和体验边界
- [x] 明确 `200 OK` 只表示 ZeroClaw 已接单，不表示任务已完成
- [x] 用真实 `.env.local` 完整启动并完成端到端验证
- [x] 明确 V1 交付形态为脚本化安装，不要求菜单栏宿主能力
- [x] 补齐“只读调查 / 审计”能力：采用 `skill + http_request + 本地只读 HA 审计代理`
- [x] 明确当前官方 Home Assistant MCP 的能力边界：当前更接近“当前状态读取 + Assist 控制”，不自带历史审计能力
- [x] 已确认 `DAVIS_HA_TOKEN` 可读取 Home Assistant REST `config/logbook/history` 接口；此前的 `ha_auth_failed` 是 Cloudflare/WAF 拦截 Python 默认 `User-Agent` 导致的
- [x] 验证 Home Assistant 侧 `recorder/history/logbook` 已为目标实体保留足够数据
- [x] 为调查类请求补齐实体解析：`main_bedroom_on_off` 可解析到 `binary_sensor.main_bedroom_on_off`，`主卧空调` 可解析到 `climate.main_bedroom`
- [x] 为调查类请求建立硬约束：只允许通过 `http_request` 访问本地只读审计代理
- [x] 增加调查类验收场景：`2026-03-23 19:20 - 20:20` 的主卧空调反复开关场景已验证

## 不纳入 V1

- [x] 不把 Siri / HomePod 异步播报当成 MVP 成功条件
- [ ] 外网访问、云中继、内网穿透能力延后到后续版本
- [ ] 家庭状态图谱、习惯学习、主动建议只保留底层 memory 能力，不纳入 V1 验收
- [ ] 外卖等跨应用深度服务暂以 Shortcut 编排 / URL Scheme 兜底为主，不单独扩展为 V1 阻塞项
- [ ] `ComputerUseTool`、桌面视觉自动化、iPhone 镜像联动明确属于 V2+
