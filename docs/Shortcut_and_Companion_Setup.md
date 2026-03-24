# Shortcut 补充说明

README 已覆盖主流程，这份文档只补充 Shortcut 侧的配置细节。

## 可直接导入的模板

仓库里现在保留 Shortcut 模板源码与重建脚本：

- 源模板：`shortcuts/叫下戴维斯.shortcut.json`
- 重建脚本：`scripts/build_shortcut.sh`

建议流程：

1. 先执行 `./scripts/build_shortcut.sh`，确保签出的 `.shortcut` 与当前源码一致
2. 脚本会生成 `shortcuts/叫下戴维斯.shortcut`
3. 把这个 `.shortcut` 文件打开或传到 iPhone 上导入
4. 导入时把 `Webhook` 地址改成你的 `http://<mac-ip>:3001/shortcut`

这份模板已经固定为：

1. `语音/文本输入`
2. `JSON POST 请求`
3. `接单确认朗读`

也就是说，它的目标是稳定完成“把自然语言请求交给 DavisZeroClaw”这件事，而不是在 Shortcut 同步阶段做复杂编排。

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

## 模板动作顺序

模板当前固定为这 3 个动作：

1. `询问输入`
2. `获取 URL 内容`
3. `朗读文本`

对应逻辑：

- `询问输入`：接收语音或文本
- `获取 URL 内容`：固定使用 `POST + JSON request body`
- `朗读文本`：固定播报 `正在处理`

模板里的 JSON 请求体等价于：

```json
{
  "sender": "ios-shortcuts",
  "content": "<用户输入>",
  "thread_id": "iphone-shortcuts"
}
```

`获取 URL 内容` 的关键配置：

- 方法：`POST`
- 请求体：`JSON`
- URL：`http://<mac-ip>:3001/shortcut`

`朗读文本` 固定为：

```text
正在处理
```

原因是这个接口的 `200 OK` 只表示 ZeroClaw 已接单。

## MVP 后续动作边界

当前 MVP 对 Shortcut 的边界明确为：

- 同步阶段必须能发送结构化 JSON 请求
- 同步阶段默认只做“接单确认朗读”
- 不要求等待 ZeroClaw 真正执行完成后再播报

如果后续 ZeroClaw 增加结构化同步返回，Shortcut 可以继续在 `获取 URL 内容` 后面追加动作，例如：

- 读取 `speech` 字段并朗读
- 读取 `open_url` 字段并跳转 App 或网页
- 读取 `handoff` 字段决定继续朗读还是跳转

建议把这类动作都加在模板的第 2 步后面，不要改动前面的输入和 JSON 请求结构。这样可以保证 Siri / Shortcut 入口始终稳定，而把后续体验增强控制在一个清晰的扩展点里。

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
