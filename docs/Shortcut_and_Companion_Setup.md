# Shortcut 补充说明

README 已覆盖主流程，这份文档只补充 Shortcut 侧的配置细节。

## Shortcut 请求格式

- URL：`http://<mac-ip>:3001/shortcut`
- Method：`POST`
- Content-Type：`application/json`

请求体：

```json
{
  "sender": "ios-shortcuts",
  "content": "关闭书房灯带",
  "thread_id": "iphone-shortcuts"
}
```

字段建议：

- `sender` 固定为 `ios-shortcuts`
- `content` 放自然语言命令
- `thread_id` 固定为一个稳定值，便于保留上下文

## `thread_id` 是做什么的

`thread_id` 可以理解成这条消息所属的“会话名”。

它的作用不是鉴权，也不是设备唯一标识，而是让 ZeroClaw 知道：

- 哪些消息属于同一段连续对话
- 哪些消息应该共享上下文

如果多次请求使用同一个 `thread_id`，ZeroClaw 会把这些请求视为同一个会话线程。这样前后的语义更容易串起来，例如前一句刚说过“书房灯带”，后一句说“把它关掉”，模型更容易知道“它”指的是谁。

如果每次都换一个新的 `thread_id`，ZeroClaw 会更像在处理一条全新的独立请求，上下文连续性会变差。

对当前这个 MVP，推荐规则很简单：

- 同一个 Shortcut，固定使用同一个 `thread_id`

推荐值：

```text
iphone-shortcuts
```

也就是说，只要你的这个 Shortcut 一直承担“给 DavisZeroClaw 发送自然语言命令”这件事，就一直复用同一个 `thread_id` 即可，不需要每次随机生成。

## Shortcut 动作顺序

推荐最小动作流：

1. `听写文本` 或 `从输入获取文本`
2. `字典`
3. `获取 URL 内容`
4. `朗读文本`

其中字典建议这样填：

- `sender = ios-shortcuts`
- `content = 上一步文本`
- `thread_id = iphone-shortcuts`

`获取 URL 内容` 建议：

- 方法：`POST`
- 请求体：`JSON`
- URL：`http://<mac-ip>:3001/shortcut`

`朗读文本` 建议固定为：

```text
正在处理
```

原因是这个接口的 `200 OK` 只表示 ZeroClaw 已接单。

## 启用 webhook.secret 时要做什么

如果你在 [config/davis/config.toml](/Users/xietian/Projects/DavisZeroClaw/config/davis/config.toml) 中启用了：

```toml
secret = "replace-with-your-shortcut-hmac-secret"
```

那么 Shortcut 还需要额外生成 HMAC-SHA256 签名，并放到请求头：

```text
x-webhook-signature
```

如果你暂时只是局域网内自用，先不启用通常更简单。

## 建议保留的体验边界

- Shortcut 同步只做“接单确认”
- 完成后的正式回执方案仍未定稿
- 不把 Siri / HomePod 异步播报当成当前 MVP 的成功条件
