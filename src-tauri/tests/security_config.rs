//! 桌面壳加固的静态断言（设计 12 §1 v0.19、ADR-0060）。
//!
//! 这些断言只读配置，不需要起 WebView：`tauri.conf.json` 与 `capabilities/default.json`
//! 是发布形态的安全边界，写错了在 CI 里没有任何症状，只在真机（或安全评审）时暴露。
//! 因此把"必须成立的性质"钉成测试——配置被改成更宽松的值时这里先红。

use serde_json::Value;

const TAURI_CONF: &str = include_str!("../tauri.conf.json");
const CAPABILITIES: &str = include_str!("../capabilities/default.json");

fn conf() -> Value {
    serde_json::from_str(TAURI_CONF).expect("tauri.conf.json 必须是合法 JSON")
}

fn capabilities() -> Value {
    serde_json::from_str(CAPABILITIES).expect("capabilities/default.json 必须是合法 JSON")
}

/// 取 CSP 里某条指令（`default-src 'self'; script-src ...` → `'self'`）。
fn directive<'a>(csp: &'a str, name: &str) -> Option<&'a str> {
    csp.split(';')
        .filter_map(|part| {
            let part = part.trim();
            part.strip_prefix(name)
                .and_then(|rest| rest.strip_prefix(' '))
                .map(str::trim)
        })
        .next()
}

#[test]
fn production_csp_denies_by_default_and_forbids_inline_script() {
    let conf = conf();
    let csp = conf["app"]["security"]["csp"]
        .as_str()
        .expect("生产 CSP 不得为 null：设计 12 §1 要求壳层加固含 CSP");

    // 兜底指令必须是 'self'：任何未列出的资源类型默认被拒。
    assert_eq!(directive(csp, "default-src"), Some("'self'"), "csp={csp}");
    // 脚本只认打包产物，不允许内联与 eval（Vite 生产构建不产内联脚本）。
    assert_eq!(directive(csp, "script-src"), Some("'self'"), "csp={csp}");
    // 一些"看起来加了 CSP、实际没什么用"的松散指令，这里逐条禁掉。
    for name in ["object-src", "base-uri"] {
        assert!(directive(csp, name).is_some(), "缺少 {name}：csp={csp}");
    }
    assert_eq!(directive(csp, "form-action"), Some("'none'"), "csp={csp}");
    assert!(!csp.contains("unsafe-eval"), "生产 CSP 不得含 unsafe-eval");
    assert!(!csp.contains("*"), "生产 CSP 不得出现通配符来源：csp={csp}");
}

#[test]
fn production_csp_keeps_only_the_style_exception_it_needs() {
    let conf = conf();
    let csp = conf["app"]["security"]["csp"].as_str().unwrap();
    let style = directive(csp, "style-src").expect("缺少 style-src");

    // 界面用 React 的 style 属性做窗口化位移（VirtualList），CSP3 下 style 属性
    // 回落 style-src，因此这一条必须有 'unsafe-inline'——它是**唯一**被允许的例外。
    assert!(style.contains("'self'"), "style-src 必须含 'self'：{style}");
    assert!(
        style.contains("'unsafe-inline'"),
        "style-src 需要 'unsafe-inline'（style 属性），实际 {style}"
    );
    // 除 style 外不允许任何指令带 unsafe-inline。
    for part in csp.split(';').map(str::trim) {
        if part.starts_with("style-src") {
            continue;
        }
        assert!(
            !part.contains("unsafe-inline"),
            "只有 style-src 可含 'unsafe-inline'，越界指令：{part}"
        );
    }
}

#[test]
fn production_csp_keeps_ipc_reachable_for_the_shell() {
    let conf = conf();
    let csp = conf["app"]["security"]["csp"].as_str().unwrap();
    let connect = directive(csp, "connect-src").expect("缺少 connect-src");

    // 桌面 IPC 走自定义协议：Linux/macOS 为 `ipc:`，Windows 为 `http://ipc.localhost`。
    assert!(connect.contains("ipc:"), "connect-src 需要 ipc:：{connect}");
    assert!(
        connect.contains("http://ipc.localhost"),
        "connect-src 需要 http://ipc.localhost（Windows IPC）：{connect}"
    );
    // 生产不得放开任意远端：除 IPC 与 'self' 外不应有 http(s) 来源。
    let remote = connect
        .split_whitespace()
        .filter(|src| src.starts_with("http://") || src.starts_with("https://"))
        .filter(|src| *src != "http://ipc.localhost")
        .collect::<Vec<_>>();
    assert!(
        remote.is_empty(),
        "connect-src 出现未预期的远端来源：{remote:?}"
    );
}

#[test]
fn dev_csp_is_separate_and_only_adds_what_vite_dev_needs() {
    let conf = conf();
    let prod = conf["app"]["security"]["csp"].as_str().unwrap();
    let dev = conf["app"]["security"]["devCsp"].as_str().expect(
        "缺少 devCsp：开发期需要给 Vite 的内联脚本与 HMR websocket 开口子，但不得污染生产 CSP",
    );

    assert_ne!(dev, prod, "devCsp 与 csp 必须分开");
    // Vite dev 注入内联的 react-refresh 预置脚本，开发期放行内联脚本是必要代价。
    let script = directive(dev, "script-src").expect("devCsp 缺少 script-src");
    assert!(
        script.contains("'unsafe-inline'"),
        "devCsp script-src={script}"
    );
    assert!(!dev.contains("unsafe-eval"), "devCsp 也不需要 unsafe-eval");
    // HMR 走同端口 websocket。
    let connect = directive(dev, "connect-src").expect("devCsp 缺少 connect-src");
    assert!(
        connect.contains("ws://localhost:1420"),
        "devCsp connect-src 需要 ws://localhost:1420：{connect}"
    );
    // 开发期也不许放开远端来源，避免"开发能跑、发布被拦"之外的相反情形。
    let remote = connect
        .split_whitespace()
        .filter(|src| src.starts_with("http://") && *src != "http://ipc.localhost")
        .collect::<Vec<_>>();
    assert!(
        remote.is_empty(),
        "devCsp connect-src 出现远端来源：{remote:?}"
    );
}

#[test]
fn capabilities_stay_at_the_minimum_the_shell_needs() {
    let caps = capabilities();
    assert_eq!(caps["identifier"], "default", "本仓库只应有一个默认能力集");
    let windows: Vec<&str> = caps["windows"]
        .as_array()
        .expect("capabilities.windows 必须是数组")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(windows, vec!["main"], "能力集只作用于主窗口");

    let permissions: Vec<&str> = caps["permissions"]
        .as_array()
        .expect("capabilities.permissions 必须是数组")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    // 最小权限：只给 Tauri core 的默认集，不装用不到的能力。
    assert_eq!(
        permissions,
        vec!["core:default"],
        "capabilities 不得扩张到 core:default 之外（加能力须先改 ADR-0060）"
    );
    // 逐项判前缀，而不是扫整份文本：`description` 里出现 "shell:" 之类字样不该误报。
    for permission in &permissions {
        assert!(
            permission.starts_with("core:"),
            "capabilities 出现未评审的插件权限：{permission}（加能力须先改 ADR-0060）"
        );
    }
}

#[test]
fn window_does_not_expose_the_global_tauri_object() {
    let conf = conf();
    // 前端只经 `lib/ipc` 的封装调用命令（ADR-0004），不靠 `window.__TAURI__` 直连。
    assert_eq!(
        conf["app"]["withGlobalTauri"],
        Value::Bool(false),
        "withGlobalTauri 必须为 false：暴露全局对象等于给页面任意脚本发命令的能力"
    );
    assert_eq!(
        conf["app"]["security"]["freezePrototype"],
        Value::Bool(true),
        "freezePrototype 必须为 true（ADR-0002 待办②的加固项）"
    );
}
