//! 网络出口层的真实链路测试：本机 DNS、直连、系统代理、自定义代理分别探测 X。
//!
//! 需要网络（且能访问 x.com），默认跳过。运行：
//!   cargo test --no-default-features --test proxy_live -- --ignored --nocapture
//!
//! 想验证自定义代理，先设 `VF_TEST_PROXY`（例如 `socks5h://127.0.0.1:7897`）：
//!   $env:VF_TEST_PROXY="socks5h://127.0.0.1:7897"; cargo test ... -- --ignored --nocapture
//!
//! 这个用例是「不开 TUN 也能解析 X」的验收依据：把 TUN 关掉、只留系统代理或
//! 自定义 SOCKS5H，再跑一次即可确认。

use videoflow_lib::net::diag;
use videoflow_lib::net::proxy::{ProxyConfig, ProxyMode};

fn custom_config() -> Option<ProxyConfig> {
    let url = std::env::var("VF_TEST_PROXY").ok()?;
    if url.trim().is_empty() {
        return None;
    }
    Some(ProxyConfig {
        mode: ProxyMode::Custom,
        custom_url: url,
    })
}

#[tokio::test]
#[ignore = "需要网络访问，默认跳过"]
async fn 三条出口路径分别探测_x() {
    let direct = ProxyConfig {
        mode: ProxyMode::Direct,
        custom_url: String::new(),
    };
    let system = ProxyConfig {
        mode: ProxyMode::System,
        custom_url: String::new(),
    };

    for (label, cfg) in [("直连", &direct), ("系统代理", &system)] {
        let probe = diag::probe_target(cfg).await;
        println!(
            "{label}：available={} detail={}",
            probe.available, probe.detail
        );
    }

    if let Some(custom) = custom_config() {
        let probe = diag::probe_target(&custom).await;
        println!(
            "自定义代理（{}）：available={} detail={}",
            videoflow_lib::net::proxy::redact(&custom.custom_url),
            probe.available,
            probe.detail
        );
        assert!(
            probe.available,
            "设了 VF_TEST_PROXY 就该能通过它访问 X：{}",
            probe.detail
        );

        // 代理端口本身必须可连接（诊断面板的「代理端口」一行）
        let endpoint = diag::probe_proxy_endpoint(&custom).await.expect("应给出端口探测");
        println!("代理端口：available={} detail={}", endpoint.available, endpoint.detail);
        assert!(endpoint.available, "代理端口应当可连接：{}", endpoint.detail);
    } else {
        println!("未设置 VF_TEST_PROXY，跳过自定义代理路径");
    }
}

#[tokio::test]
#[ignore = "需要网络访问，默认跳过"]
async fn 完整诊断给出结论() {
    let cfg = custom_config().unwrap_or(ProxyConfig {
        mode: ProxyMode::System,
        custom_url: String::new(),
    });
    let report = diag::diagnose(&cfg).await;

    println!("模式：{}（{}）", report.mode_label, report.mode);
    println!("生效代理：{:?}", report.active_proxy);
    println!("本机 DNS：{}", report.dns.detail);
    println!("直连：{}", report.direct.detail);
    println!("系统代理：{}", report.system.detail);
    println!("自定义代理：{}", report.custom.detail);
    println!("结论：{}（proxyOk={}）", report.verdict, report.proxy_ok);

    assert!(!report.verdict.is_empty(), "结论不能为空");
    // 结论必须能回答「能不能访问 X」这个问题
    assert!(!report.dns.detail.is_empty());
}
