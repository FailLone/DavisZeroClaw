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

第一次查询淘宝或京东包裹前，先在 Mac 上登录对应网页：

```bash
daviszeroclaw express login ali
```

```bash
daviszeroclaw express login jd
```

登录后再问 Davis 查询快递即可。

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
```

## 隐私说明

Davis 默认运行在你的 Mac 上。本地配置、运行状态和浏览器读取结果都保存在这台电脑里。

请不要把 `config/davis/local.toml` 发给别人，因为里面会有你的 Home Assistant 访问密钥和大模型服务访问密钥。
