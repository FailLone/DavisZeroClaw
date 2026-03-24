# DavisZeroClaw

DavisZeroClaw 是一个跑在 macOS 上的轻量级家庭 AI 管家。它基于官方 [ZeroClaw](https://zeroclaws.io/) 运行时，把 `Siri / iOS Shortcut` 和 `Home Assistant MCP` 串成一条可用的本地控制链路。

当前这套仓库聚焦一件事：让你在自己的设备上尽快跑通这条链路。

## 这套方案现在能做什么

- 用 iPhone Shortcut 通过 `Webhook Channel` 把自然语言命令发给 ZeroClaw
- 让 ZeroClaw 通过 Home Assistant MCP 调用设备控制
- 通过本地只读审计代理 + `ha-audit` skill，调查某个 Home Assistant 实体最近一段时间内的状态变化

当前不追求的内容：

- 不使用官方 no-tools 的 `POST /webhook` 作为 Shortcut 主入口
- 不承诺 Siri / HomePod 在异步完成后主动播报结果
- 不直接改官方 ZeroClaw 源码

## 运行前准备

你需要准备好以下东西：

1. 一台 macOS 主机
2. Homebrew
3. 已安装并可访问的 Home Assistant
4. Home Assistant 中已启用 MCP Server
5. 一枚 Home Assistant Long-Lived Access Token
6. 一个模型 API Key
   默认模板使用 `OpenRouter`，因此需要 `OPENROUTER_API_KEY`
7. iPhone，供 Shortcut 发起请求

## 第一步：安装 ZeroClaw

```bash
brew install zeroclaw
```

安装后确认可用：

```bash
zeroclaw --version
```

## 第二步：准备环境变量

复制示例文件：

```bash
cp .env.example .env.local
```

编辑 `.env.local`，至少填这三个值：

```bash
OPENROUTER_API_KEY=your_openrouter_api_key
DAVIS_HA_URL=http://homeassistant.local:8123/api/mcp
DAVIS_HA_TOKEN=your_home_assistant_long_lived_access_token
```

说明：

- `DAVIS_HA_URL` 是 Home Assistant MCP 地址
- `DAVIS_HA_TOKEN` 是 Home Assistant 的 Long-Lived Access Token
- `OPENROUTER_API_KEY` 是默认模型路由所需的 API Key

## 第三步：检查配置模板

主模板在 [config/davis/config.toml](/Users/xietian/Projects/DavisZeroClaw/config/davis/config.toml)。

当前模板已经包含这条 MVP 所需的最小配置：

- `gateway` 监听 `3000`
- `channels_config.webhook` 监听 `3001/shortcut`
- `http_request` 只放行 `127.0.0.1/localhost`
- `mcp.servers.homeassistant` 通过 SSE 连接 Home Assistant MCP

如果你只是想跑通当前主链路，通常不需要改它。

如果你要增强安全性，可以再看这一项：

- `channels_config.webhook.secret`
  默认未启用；启用后，Shortcut 需要额外携带 HMAC 签名

## 第四步：启动 DavisZeroClaw

```bash
./scripts/start_davis.sh
```

这个脚本会做三件事：

1. 读取 `.env.local` 或 `.env`
2. 渲染运行时配置到 `.runtime/davis/config.toml`
3. 启动一个本地只读 HA 审计代理和两个官方进程

- `python3 scripts/ha_audit_proxy.py`
- `zeroclaw gateway start`
- `zeroclaw channel start`

这里刻意没有直接依赖 `zeroclaw daemon`，因为在 `zeroclaw 0.5.7` 下，`daemon` 不会实际拉起 `channels_config.webhook` 的监听。

启动成功后，你应该有两个入口：

- `http://<mac-ip>:3000/health`
- `http://<mac-ip>:3001/shortcut`
- `http://127.0.0.1:3010/health`

停止服务：

```bash
./scripts/stop_davis.sh
```

## 第五步：配置 iPhone Shortcut

Shortcut 应该向下面这个地址发请求：

```json
POST http://<mac-ip>:3001/shortcut
Content-Type: application/json

{"sender":"ios-shortcuts","content":"关闭书房灯带","thread_id":"iphone-shortcuts"}
```

推荐在 Shortcut 里这样组织动作：

1. 获取语音输入或文本输入
2. 组装一个字典
   - `sender = ios-shortcuts`
   - `content = 上一步文本`
   - `thread_id = iphone-shortcuts`
3. 使用“获取 URL 内容”发起 `POST`
4. 在收到 `200 OK` 后朗读“正在处理”

这里的 `200 OK` 只表示 ZeroClaw 已接单，不表示最终动作已经执行完成。

更细的 Shortcut 配置细节见 [docs/Shortcut_and_Companion_Setup.md](/Users/xietian/Projects/DavisZeroClaw/docs/Shortcut_and_Companion_Setup.md)。

## 第五点五步：安装 `ha-audit` Skill

如果你希望 DavisZeroClaw 能调查“谁动了这个实体”或“为什么最近一小时反复开关”，还需要安装仓库内置的 `ha-audit` skill：

```bash
zeroclaw skills install ./skills/ha-audit
```

安装后可检查：

```bash
zeroclaw skills list
```

这个 skill 不会直接访问 Home Assistant 写接口。它只会通过 ZeroClaw 的 `http_request` 工具访问本机只读代理：

```text
http://127.0.0.1:3010/audit
```

代理内部会使用你现有的 `DAVIS_HA_URL` 和 `DAVIS_HA_TOKEN` 去查询 Home Assistant 的 `logbook` 和 `history`。

## 第六步：做一次端到端测试

先确认 ZeroClaw 已经启动，再在本机执行：

```bash
curl -i -X POST http://127.0.0.1:3001/shortcut \
  -H 'Content-Type: application/json' \
  -d '{"sender":"ios-shortcuts","content":"打开书房灯带","thread_id":"iphone-shortcuts"}'
```

预期结果：

1. 返回 `HTTP/1.1 200 OK`
2. ZeroClaw 处理命令
3. Home Assistant 中对应实体状态变化

只读调查类测试示例：

```bash
curl -i -X POST http://127.0.0.1:3001/shortcut \
  -H 'Content-Type: application/json' \
  -d '{"sender":"ios-shortcuts","content":"请调查最近一小时内 switch.main_bedroom_on_off 为什么反复开关，找出是谁操作的。只允许读取，不要执行任何写操作。","thread_id":"iphone-shortcuts"}'
```

这类请求应该触发 `ha-audit` skill，而不是直接调用 Home Assistant 写工具。

## 文件说明

- [config/davis/config.toml](/Users/xietian/Projects/DavisZeroClaw/config/davis/config.toml)：ZeroClaw 配置模板
- [skills/ha-audit/SKILL.md](/Users/xietian/Projects/DavisZeroClaw/skills/ha-audit/SKILL.md)：Home Assistant 只读审计 skill
- [scripts/start_davis.sh](/Users/xietian/Projects/DavisZeroClaw/scripts/start_davis.sh)：启动脚本
- [scripts/ha_audit_proxy.py](/Users/xietian/Projects/DavisZeroClaw/scripts/ha_audit_proxy.py)：本地只读 HA 审计代理
- [scripts/stop_davis.sh](/Users/xietian/Projects/DavisZeroClaw/scripts/stop_davis.sh)：停止脚本
- [docs/Shortcut_and_Companion_Setup.md](/Users/xietian/Projects/DavisZeroClaw/docs/Shortcut_and_Companion_Setup.md)：Shortcut 的补充说明

## 常见问题

`为什么不用 gateway /webhook？`

因为官方通用 `POST /webhook` 是 simple chat 入口，不会稳定进入工具调用链；Shortcut 主入口必须走 `channels_config.webhook`。

`为什么要起两个进程？`

因为当前验证过的稳定路径是：

- `zeroclaw gateway start`
- `zeroclaw channel start`

而不是只起一个 `zeroclaw daemon`。

`如果我改了 .env.local 或 config 模板怎么办？`

重新运行：

```bash
./scripts/start_davis.sh
```

脚本会重新渲染 `.runtime/davis/config.toml`。
