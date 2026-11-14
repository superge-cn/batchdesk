//! S01-PRE-05 真共享三态实机探针（Windows 实机日专用；默认跳过，不进 CI）。
//!
//! 为什么不能进 CI：**可达态**需要一个真实存在的 SMB 共享，CI runner 不一定有；
//! 本测试因此由环境变量 `BATCHDESK_UNC_SHARE` 驱动——未设时只打印跳过说明并返回，
//! 设了才探测。可达（`Some(true)`）／明确不可达（`Some(false)`）／超时（`None`）
//! 三态由 kernel 纯函数映射为 Pass／Warn+E2005／Warn+E2005（方案 §4.9：都只降级为
//! 提示、不整任务阻断）。
//!
//! 用法（需管理员）：
//!   New-SmbShare -Name BatchDeskS01 -Path E:\tmp\s01share -FullAccess Everyone
//!   BATCHDESK_UNC_SHARE='//localhost/BatchDeskS01' `
//!     BATCHDESK_UNC_SLOW_HOST='//10.255.255.1/share' `
//!     cargo test -p batchdesk-platform --test unc_real_probe -- --nocapture
//!   Remove-SmbShare -Name BatchDeskS01 -Force
//!
//! `BATCHDESK_UNC_SLOW_HOST` 可选：用来验「慢速＝超时」那条降级口径。

/// 探测并打印一个网络路径的可达性，返回 `probe_unc` 的原始结果。
#[cfg(windows)]
fn probe(label: &str, path: &str) -> kernel::preflight::UncCheck {
    use std::path::Path;
    use std::time::Instant;

    let started = Instant::now();
    let check = platform::preflight::probe_unc(Path::new(path))
        .expect("网络路径应被探测（非网络路径返回 None）");
    println!(
        "[{label}] {path} => reachable={:?}（{} ms）",
        check.reachable,
        started.elapsed().as_millis()
    );
    check
}

#[cfg(windows)]
#[test]
fn real_unc_share_is_reachable_and_missing_hosts_warn() {
    let Some(share) = std::env::var("BATCHDESK_UNC_SHARE").ok() else {
        println!(
            "跳过：未设 BATCHDESK_UNC_SHARE（先用管理员建好本机共享，如 //localhost/BatchDeskS01）"
        );
        return;
    };
    let share = share.trim().trim_end_matches('/');

    // ① 可达态：真实共享根必须探成 Some(true)。这是本条缺的那一半——
    //    既有单测只覆盖不可达，若探测口径把可达共享也判成不可达，
    //    预检会对正常网络路径误报 E2005。
    let check = probe("可达", share);
    assert_eq!(
        check.reachable,
        Some(true),
        "真实共享必须探为可达；为 false/None 说明探测口径或共享配置有问题"
    );

    // ② 明确不可达：`.invalid` 保留 TLD 不解析（RFC 2606），任何机器都复现。
    let missing = "//batchdesk-invalid-host.invalid/share";
    let check = probe("不可达", missing);
    assert_ne!(check.reachable, Some(true), "不存在的 host 不得探为可达");

    // ③ 超时态（慢速）：不可路由地址，UNC_PROBE_TIMEOUT 内应为 None（暂不可达）。
    if let Ok(slow) = std::env::var("BATCHDESK_UNC_SLOW_HOST") {
        let slow = slow.trim();
        let check = probe("超时", slow);
        assert_eq!(
            check.reachable,
            None,
            "不可路由地址应在 {:?} 内超时为 None（＝方案 §4.9 的慢速降级口径）",
            platform::preflight::UNC_PROBE_TIMEOUT
        );
    }
}
