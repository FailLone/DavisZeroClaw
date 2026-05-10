# DavisZeroClaw

DavisZeroClaw 是一个跑在 Mac 上的家庭 AI 管家。

你可以对 iPhone 说一句话，让它帮你控制家里的设备、查询家里的状态，或者看看最近的快递。它不需要单独的手机 App，入口就是你熟悉的 Siri、快捷指令和 iMessage。

## 可以做什么

- 打开、关闭家里的灯和开关
- 查询某个设备现在是什么状态
- 询问家里设备最近发生了什么
- 用 iPhone 快捷指令唤起 Davis
- 用 iMessage 给自己的 Mac 发消息唤起 Davis
- 查询淘宝、京东最近的包裹
- 给 Home Assistant 的命名、别名和分组提供整理建议

## 你需要准备

- 一台 Mac
- Homebrew，也就是 Mac 上常用的安装工具
- Home Assistant
- Home Assistant 给 Davis 使用的访问密钥
- 一个大模型服务的访问密钥
- 一台 iPhone

## 第一次启动

先安装 ZeroClaw：

```bash
brew install zeroclaw
```

准备 Davis 命令：

```bash
cargo build --release --bin daviszeroclaw
export PATH="$PWD/target/release:$PATH"
```

复制配置文件：

```bash
cp config/davis/local.example.toml config/davis/local.toml
```

打开 `config/davis/local.toml`，填好这些内容：

- Home Assistant 地址
- Home Assistant 访问密钥
- 大模型服务访问密钥
- Davis 和快捷指令之间的暗号
- iMessage 允许的手机号或邮箱

如果不确定 iMessage 应该填什么，可以运行：

```bash
daviszeroclaw imessage inspect
```

它会告诉你当前 Mac 登录的 Messages 账号，并给出建议填入配置的手机号或邮箱。

## 启动 Davis

```bash
daviszeroclaw start
```

启动成功后，Davis 会在这台 Mac 上常驻运行。

如果要停止：

```bash
daviszeroclaw stop
```

如果你希望把 ZeroClaw 以 Davis 配置常驻到 macOS 后台服务里，使用：

```bash
daviszeroclaw service install
```

查看最终状态：

```bash
daviszeroclaw service status
```

更新配置后重启服务：

```bash
daviszeroclaw service restart
```

查看日志时优先使用统一入口：

```bash
daviszeroclaw logs
daviszeroclaw logs --follow
daviszeroclaw logs --component crawl4ai
daviszeroclaw logs --component router-dhcp
daviszeroclaw logs --clear
```

`logs` 会显示它正在读取哪些 runtime 日志文件。一般排障先看
`daviszeroclaw service status`，再按提示运行对应的 `daviszeroclaw logs ...`
命令；不需要手动猜 `.runtime/davis/` 里哪个 `.log` 最重要。

日志轮转使用日期后缀，例如 `proxy.launchd.stderr.log.2026-05-10`；
同一天多次轮转会追加时间。Davis 在启动或重启服务前自动轮转超过 10MB 的日志，
并清理超过 3 天的归档。`daviszeroclaw logs --clear` 会清空当前选择的日志并删除它们的归档。

如果 ZeroClaw 的 iMessage channel 因为无法读取
`~/Library/Messages/chat.db` 而反复重启，Davis 会在渲染 runtime config 时
自动禁用该 channel，避免刷日志。修复权限后运行：

```bash
daviszeroclaw imessage check-permissions
daviszeroclaw service restart
```

卸载后台服务：

```bash
daviszeroclaw service uninstall
```

这里的 `service` 只管理 ZeroClaw daemon。最终结果以 `daviszeroclaw service status` 为准。
如果你没有把 `target/release` 加进 `PATH`，也可以直接运行 `target/release/daviszeroclaw ...`。

## 安装 iPhone 快捷指令

运行：

```bash
daviszeroclaw shortcut install
```

命令会生成适合当前 Mac 的快捷指令，并打开系统导入界面。macOS 和 iOS 仍会要求你确认导入，这是正常的安全步骤。

如果你只想生成文件，不自动打开导入界面：

```bash
daviszeroclaw shortcut build
```

## 日常使用

可以对 Siri 或快捷指令说：

```text
打开书房灯带
```

```text
关闭父母间吊灯
```

```text
书房灯带现在开着吗
```

```text
帮我查一下最近的快递
```

快捷指令听到“正在处理”，只代表 Davis 已经收到请求，不代表设备动作已经完成。

## 快递查询

第一次使用 Crawl4AI 前，先安装并检查运行时：

```bash
daviszeroclaw crawl install
```

```bash
daviszeroclaw crawl check
```

第一次查询淘宝或京东包裹前，再在 Mac 上登录对应网页：

```bash
daviszeroclaw crawl profile login express-ali
```

```bash
daviszeroclaw crawl profile login express-jd
```

命令会打开一个 `Crawl4AI` 兼容的持久浏览器 profile。完成登录后，回到终端按一次回车保存并结束登录流程；后续查询会复用这份登录态，不依赖你日常正在使用的 Chrome 标签页。
现在 `express` 的读取链路已经走 `Crawl4AI`，登录 helper 只负责初始化持久 profile。

查看内建 crawl source：

```bash
daviszeroclaw crawl source list
```

直接运行包裹抓取：

```bash
daviszeroclaw crawl run express-packages --refresh
```

## Skills、Crawl4AI 和 MemPalace

Davis 自己维护一部分 project skills，也支持安装第三方 vendor skills。

同步 runtime skills：

```bash
daviszeroclaw skills sync
```

安装或刷新当前支持的 vendor skills：

```bash
daviszeroclaw skills install
```

检查 project skills、vendor skills、runtime sync 状态，以及 MemPalace MCP 可用性：

```bash
daviszeroclaw skills check
```

同步 runtime SOP：

```bash
daviszeroclaw sops sync
```

检查 SOP 是否已同步并通过 ZeroClaw 校验：

```bash
daviszeroclaw sops check
```

如果你想把 MemPalace 接进 Davis：

```bash
daviszeroclaw memory mempalace install
daviszeroclaw memory mempalace enable
daviszeroclaw memory mempalace check
```

其中：

- `skills/crawl4ai` 负责告诉 agent 如何维护 Crawl4AI 本身，以及如何使用 Davis 的统一 `crawl` 命令面
- `skills/mempalace` 负责告诉 agent 如何操作 MemPalace
- `project-skills/mempalace-memory` 负责告诉 agent 什么时候应优先把 MemPalace 当作长期 memory
- `project-skills/my-parcels` 负责告诉 agent 如何通过本地 Davis proxy 安全查询快递，不直接去外部购物网站抓取
- `project-sops/` 是用户自定义 ZeroClaw runbook（SOP）的目录，默认为空；要新增 SOP 请看 `project-sops/README.md`

## 常用命令

```bash
daviszeroclaw config check
daviszeroclaw ha check
daviszeroclaw imessage inspect
daviszeroclaw shortcut build
daviszeroclaw shortcut install
daviszeroclaw crawl install
daviszeroclaw crawl check
daviszeroclaw crawl source list
daviszeroclaw crawl run express-packages --refresh
daviszeroclaw crawl profile login express-ali
daviszeroclaw crawl profile login express-jd
daviszeroclaw sops sync
daviszeroclaw sops check
```

## 常见问题

如果控制失败，先检查：

- Home Assistant 是否能正常访问
- `config/davis/local.toml` 是否填对
- Mac 和 iPhone 是否在同一个可访问的网络里
- 快捷指令里的暗号是否和配置一致
- 启动 Davis 的终端是否有 Messages 所需权限

可以用这些命令做基础检查：

```bash
daviszeroclaw config check
daviszeroclaw ha check
daviszeroclaw imessage inspect
daviszeroclaw service status
```

## 隐私说明

Davis 默认运行在你的 Mac 上。本地配置、运行状态和浏览器读取结果都保存在这台电脑里。

请不要把 `config/davis/local.toml` 发给别人，因为里面会有你的 Home Assistant 访问密钥和大模型服务访问密钥。
