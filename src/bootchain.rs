//! bootchain — проверка системного образа после старта ядра.
//!
}

fn hex8(digest: &[u8; 32]) -> String {
    let mut s = String::new();
    for b in digest.iter().take(4) {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

pub fn run_boot_chain() -> Result<String, String> {
    let mut out = String::new();
    out.push_str("  [bootchain] Проверка системного образа (/system, EROFS):\n");

            crate::diag::error(
                crate::diag::ErrorCode::new(crate::diag::Subsystem::Kernel, 13),
                &e,
            );
            out.push_str(&format!("    сверка образов ядра не выполнена: {}\n", e));
        }
    }

    out.push_str("  [bootchain] Системный образ проверен.\n");
    Ok(out)
}
