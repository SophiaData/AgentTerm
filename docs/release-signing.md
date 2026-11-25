# 发布签名与校验

测试版只校验 GitHub 提供的 SHA-256，正式版必须再加一层可验证的发布者身份。
**这是 `0.2.0` 正式发布前的阻断项**：拿到证书之前不发正式版。

## 现状

| 层面       | 现在做的                                                            | 缺口                               |
| ---------- | ------------------------------------------------------------------- | ---------------------------------- |
| 传输       | 只允许 `https://github.com/SophiaData/AgentTerm/releases/download/` | —                                  |
| 完整性     | 安装前用 GitHub API 的 `sha256` 摘要校验下载内容                    | —                                  |
| 来源版本   | 只安装当前版本允许升级到的那个资产                                  | —                                  |
| 发布者身份 | 无                                                                  | Windows 未签名，macOS 未签名未公证 |

摘要能防篡改，防不了“发布账号被盗后重新上传一个自洽的包”。签名只能由证书持有者
产生，才能补上这一段。

## Windows

1. 准备代码签名证书（EV 或 OV），导入到当前用户的证书存储。
2. 在构建机设置环境变量（**只写变量名，值不进仓库**）：
   `TAURI_SIGNING_PRIVATE_KEY` 仅在启用 Tauri 更新签名时使用；
   Authenticode 通过证书指纹引用，指纹写在 CI Secret 里，例如 `WINDOWS_CERT_THUMBPRINT`。
3. `tauri.conf.json` 增加：

   ```json
   "bundle": {
     "windows": {
       "certificateThumbprint": "%WINDOWS_CERT_THUMBPRINT%",
       "digestAlgorithm": "sha256",
       "timestampUrl": "http://timestamp.digicert.com"
     }
   }
   ```

4. 构建后在 CI 里执行 `signtool verify /pa /v <installer>`，输出必须为
   `Successfully verified`，并把结果贴进 release 说明。
5. 客户端安装前可选做一次 Authenticode 校验（`WinVerifyTrust`），失败即拒绝安装。

## macOS

1. 需要 Apple Developer ID Application 证书与 `notarytool` 凭据，凭据存 CI Secret
   （`APPLE_ID`、`APPLE_TEAM_ID`、`APPLE_APP_PASSWORD`，值不进仓库）。
2. 构建后依次执行 `codesign --deep --options runtime`、`xcrun notarytool submit --wait`、
   `xcrun stapler staple`。
3. 分发前用 `spctl -a -vv` 校验，必须显示 `accepted`。
4. 客户端在挂载 DMG 前执行 `spctl --assess`，失败即拒绝。

## 发布检查清单

- [ ] 安装包与 TUI 二进制都已签名，Windows 用 `signtool verify /pa` 复核
- [ ] macOS 已公证并 staple，`spctl -a -vv` 显示 accepted
- [ ] `SHA256SUMS.txt` 与 GitHub 资产摘要逐项一致
- [ ] Release 说明写明签名主体与证书到期时间
- [ ] 若本轮未签名，只发 Pre-release，并在说明中标注“未签名”

## 未完成前的口径

测试包（`x.y.z-rc.N`）一律标注“未签名”，只发 Pre-release；正式版必须等上表全部
勾选完成。客户端现有的摘要校验不做妥协，签名校验是追加而不是替代。
