// services/screens.rs — 018 Fase 2 US3 (T030) — multi-monitor placement.
//
// NÚCLEO PURO de resolución de geometría de ventana sobre los monitores (displays) DISPONIBLES.
// Separado del crate-level `monitors.rs` (que es monitoreo de salud de servidores, otro dominio).
// La parte Tauri-dependiente (`available_monitors`/`primary_monitor`) vive en el comando
// `monitors_list` (commands.rs); ACÁ sólo la lógica determinista y testeable sin runtime.
//
// `DisplayHint` (layout_config) es una PISTA, no autoridad: si el `monitor_id` no existe al
// rehidratar (monitor desconectado), caemos al primario; la geometría siempre se CLAMPea para que
// la ventana quede DENTRO del monitor objetivo (anti-off-screen: nunca una ventana inalcanzable).

use serde::{Deserialize, Serialize};

use crate::services::layout_config::DisplayHint;

/// Geometría de un monitor en píxeles FÍSICOS (bounds completos del display). `id` es un id ÚNICO
/// dentro del snapshot (nombre + posición — desambigua monitores con el mismo nombre, p.ej. dos
/// displays idénticos) y razonablemente estable mientras el arreglo físico no cambie; es lo que
/// matchea `DisplayHint.monitor_id`. `scale_factor` (HiDPI) permite convertir tamaños lógicos de UI
/// a físicos para este monitor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenInfo {
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub is_primary: bool,
}

/// Geometría final a aplicar a una ventana (esquina sup-izq + tamaño), en px físicos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Tamaño mínimo de ventana, en píxeles LÓGICOS de UI (no degenerar a una ventana inusable).
/// Se convierte a físicos por monitor (× `scale_factor`) dentro de `resolve_placement`.
pub const MIN_LOGICAL_W: u32 = 480;
pub const MIN_LOGICAL_H: u32 = 360;

#[inline]
fn clamp_i32(v: i32, lo: i32, hi: i32) -> i32 {
    // hi puede quedar < lo si la ventana llena el monitor → priorizamos lo (borde izq/sup).
    v.max(lo).min(hi.max(lo))
}

#[inline]
fn to_physical(logical: u32, scale: f64) -> u32 {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    ((logical as f64) * s).round() as u32
}

/// Elige el monitor objetivo: por `hint.monitor_id` si EXISTE en `screens`; si no matchea (monitor
/// ausente/desconectado) o no hay hint → el primario (o el primero si ninguno se marca primario).
pub fn target_screen<'a>(
    hint: Option<&DisplayHint>,
    screens: &'a [ScreenInfo],
) -> Option<&'a ScreenInfo> {
    if screens.is_empty() {
        return None;
    }
    let primary = screens.iter().find(|s| s.is_primary).unwrap_or(&screens[0]);
    let chosen = hint
        .and_then(|h| h.monitor_id.as_deref())
        .and_then(|id| screens.iter().find(|s| s.id == id))
        .unwrap_or(primary);
    Some(chosen)
}

/// Resuelve dónde abrir/reubicar una ventana dado su `DisplayHint` y los monitores disponibles.
/// TODO en píxeles FÍSICOS (igual que `Monitor::position()/size()` de Tauri y que
/// `set_position`/`set_size(Physical*)`).
/// - Monitor objetivo: `target_screen` (hint o fallback a primario).
/// - Tamaño: del hint (físico, persistido de la geometría real) o defaults LÓGICOS convertidos a
///   físicos por el `scale_factor` del monitor (HiDPI). Clamp a `[MIN, tamaño del monitor]` PERO
///   priorizando "entrar en el monitor": si el monitor es más chico que el mínimo, la ventana toma
///   el tamaño del monitor (nunca lo excede → no off-screen).
/// - Posición: del hint (coords globales) o CENTRADA en el monitor; SIEMPRE clampeada para que la
///   ventana entera quede dentro del monitor objetivo (anti-off-screen, incl. monitores con x<0).
/// Devuelve `None` si NO hay monitores (dejar que el WM decida — no forzamos geometría a ciegas).
/// `default_logical_w/h` son tamaños LÓGICOS de UI. PURA → testeable sin Tauri.
pub fn resolve_placement(
    hint: Option<&DisplayHint>,
    screens: &[ScreenInfo],
    default_logical_w: u32,
    default_logical_h: u32,
) -> Option<Placement> {
    let target = target_screen(hint, screens)?;
    let scale = target.scale_factor;

    // MIN y defaults son LÓGICOS → a físicos para ESTE monitor (HiDPI). El hint ya viene en físicos
    // (se persiste de la geometría real de la ventana).
    let min_w = to_physical(MIN_LOGICAL_W, scale);
    let min_h = to_physical(MIN_LOGICAL_H, scale);
    let want_w = hint
        .and_then(|h| h.width)
        .unwrap_or_else(|| to_physical(default_logical_w, scale));
    let want_h = hint
        .and_then(|h| h.height)
        .unwrap_or_else(|| to_physical(default_logical_h, scale));

    // Clamp de tamaño a [MIN, monitor], priorizando entrar en el monitor: `.min(target.width)`
    // garantiza que NUNCA exceda el monitor (aunque sea más chico que el mínimo) → no off-screen.
    let w = want_w.max(min_w).min(target.width.max(1));
    let h = want_h.max(min_h).min(target.height.max(1));

    // Posición deseada: hint con x e y → esas coords; sino centrar en el monitor objetivo.
    let (dx, dy) = match (hint.and_then(|h| h.x), hint.and_then(|h| h.y)) {
        (Some(hx), Some(hy)) => (hx, hy),
        _ => (
            target.x + ((target.width as i32 - w as i32) / 2).max(0),
            target.y + ((target.height as i32 - h as i32) / 2).max(0),
        ),
    };

    // CLAMP anti-off-screen: la ventana entera debe quedar dentro de [target.x, target.x+width].
    // Como w ≤ target.width, max_x ≥ target.x siempre (clamp_i32 igual protege con hi.max(lo)).
    let max_x = target.x + target.width as i32 - w as i32;
    let max_y = target.y + target.height as i32 - h as i32;
    let x = clamp_i32(dx, target.x, max_x);
    let y = clamp_i32(dy, target.y, max_y);

    Some(Placement {
        x,
        y,
        width: w,
        height: h,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screens_two() -> Vec<ScreenInfo> {
        vec![
            // Primario: 1920x1080 en (0,0), scale 1.0.
            ScreenInfo {
                id: "primary".into(),
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                scale_factor: 1.0,
                is_primary: true,
            },
            // Secundario: 2560x1440 a la derecha en (1920,0), scale 1.0.
            ScreenInfo {
                id: "hdmi-1".into(),
                x: 1920,
                y: 0,
                width: 2560,
                height: 1440,
                scale_factor: 1.0,
                is_primary: false,
            },
        ]
    }

    fn hint(
        monitor: Option<&str>,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
    ) -> DisplayHint {
        DisplayHint {
            monitor_id: monitor.map(|s| s.to_string()),
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn matches_present_monitor() {
        // hint apunta al secundario presente → ventana cae en sus coords globales.
        let s = screens_two();
        let dh = hint(Some("hdmi-1"), Some(2000), Some(100), Some(800), Some(600));
        let p = resolve_placement(Some(&dh), &s, 1100, 760).unwrap();
        assert_eq!(p.width, 800);
        assert_eq!(p.height, 600);
        // x=2000 cabe en [1920, 1920+2560-800] → se respeta.
        assert_eq!(p.x, 2000);
        assert_eq!(p.y, 100);
    }

    #[test]
    fn falls_back_to_primary_when_monitor_absent() {
        // monitor_id inexistente (desconectado) → usa el primario y centra/clampa ahí.
        let s = screens_two();
        let dh = hint(Some("ghost-monitor"), None, None, Some(800), Some(600));
        let p = resolve_placement(Some(&dh), &s, 1100, 760).unwrap();
        // Centrada en el primario (0,0,1920,1080).
        assert_eq!(p.x, (1920 - 800) / 2);
        assert_eq!(p.y, (1080 - 600) / 2);
        // Y dentro del primario (no del secundario).
        assert!(p.x >= 0 && p.x + p.width as i32 <= 1920);
    }

    #[test]
    fn clamps_off_screen_position_into_target() {
        // hint con coords MUY fuera del monitor objetivo → clamp dentro del monitor.
        let s = screens_two();
        let dh = hint(
            Some("primary"),
            Some(99999),
            Some(-5000),
            Some(800),
            Some(600),
        );
        let p = resolve_placement(Some(&dh), &s, 1100, 760).unwrap();
        // Esquina sup-izq clampeada al borde derecho/superior del primario.
        assert_eq!(p.x, 1920 - 800); // max_x
        assert_eq!(p.y, 0); // min_y
                            // Ventana entera on-screen.
        assert!(p.x + p.width as i32 <= 1920);
        assert!(p.y >= 0 && p.y + p.height as i32 <= 1080);
    }

    #[test]
    fn no_hint_centers_on_primary() {
        let s = screens_two();
        let p = resolve_placement(None, &s, 1100, 760).unwrap();
        assert_eq!(p.width, 1100);
        assert_eq!(p.height, 760);
        assert_eq!(p.x, (1920 - 1100) / 2);
        assert_eq!(p.y, (1080 - 760) / 2);
    }

    #[test]
    fn clamps_size_to_min_and_to_monitor() {
        let s = screens_two();
        // Tamaño absurdamente chico → sube a MIN (scale 1.0 → físico == lógico).
        let tiny = hint(Some("primary"), Some(0), Some(0), Some(10), Some(10));
        let p = resolve_placement(Some(&tiny), &s, 1100, 760).unwrap();
        assert_eq!(p.width, MIN_LOGICAL_W);
        assert_eq!(p.height, MIN_LOGICAL_H);
        // Tamaño mayor que el monitor → baja al tamaño del monitor.
        let huge = hint(Some("primary"), Some(0), Some(0), Some(9999), Some(9999));
        let p2 = resolve_placement(Some(&huge), &s, 1100, 760).unwrap();
        assert_eq!(p2.width, 1920);
        assert_eq!(p2.height, 1080);
    }

    #[test]
    fn hidpi_converts_logical_defaults_to_physical() {
        // Monitor 2x (Retina): 3840x2160 físicos, scale 2.0. Sin hint → defaults LÓGICOS 1100x760
        // deben aplicarse como FÍSICOS 2200x1520 (no 1100, que se vería a mitad de tamaño).
        let s = vec![ScreenInfo {
            id: "retina".into(),
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
            scale_factor: 2.0,
            is_primary: true,
        }];
        let p = resolve_placement(None, &s, 1100, 760).unwrap();
        assert_eq!(p.width, 2200, "1100 lógico × 2.0 = 2200 físico");
        assert_eq!(p.height, 1520, "760 lógico × 2.0 = 1520 físico");
        // Centrada en el monitor físico.
        assert_eq!(p.x, (3840 - 2200) / 2);
        assert_eq!(p.y, (2160 - 1520) / 2);
        // MIN también escala: un tamaño chico sube a MIN_LOGICAL × 2.
        let tiny = hint(Some("retina"), Some(0), Some(0), Some(50), Some(50));
        let pt = resolve_placement(Some(&tiny), &s, 1100, 760).unwrap();
        assert_eq!(pt.width, MIN_LOGICAL_W * 2);
        assert_eq!(pt.height, MIN_LOGICAL_H * 2);
    }

    #[test]
    fn tiny_monitor_window_never_exceeds_it() {
        // Monitor MÁS CHICO que el mínimo (300x200 < 480x360). La ventana debe tomar el tamaño del
        // monitor (entrar entero), NUNCA excederlo (anti-off-screen), sin panic por clamp invertido.
        let s = vec![ScreenInfo {
            id: "tiny".into(),
            x: 0,
            y: 0,
            width: 300,
            height: 200,
            scale_factor: 1.0,
            is_primary: true,
        }];
        let p = resolve_placement(None, &s, 1100, 760).unwrap();
        assert_eq!(p.width, 300, "no excede el ancho del monitor");
        assert_eq!(p.height, 200, "no excede el alto del monitor");
        // Ventana entera dentro del monitor.
        assert!(p.x >= 0 && p.x + p.width as i32 <= 300);
        assert!(p.y >= 0 && p.y + p.height as i32 <= 200);
    }

    #[test]
    fn clamps_into_monitor_with_negative_offset() {
        // Monitor a la IZQUIERDA del primario (x<0). Un hint fuera de rango se clampa dentro de él.
        let s = vec![
            ScreenInfo {
                id: "left".into(),
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080,
                scale_factor: 1.0,
                is_primary: false,
            },
            ScreenInfo {
                id: "primary".into(),
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                scale_factor: 1.0,
                is_primary: true,
            },
        ];
        let dh = hint(
            Some("left"),
            Some(-99999),
            Some(99999),
            Some(800),
            Some(600),
        );
        let p = resolve_placement(Some(&dh), &s, 1100, 760).unwrap();
        // Clamp al borde izq (x = -1920) y borde inf (y = 1080-600).
        assert_eq!(p.x, -1920);
        assert_eq!(p.y, 1080 - 600);
        assert!(p.x >= -1920 && p.x + p.width as i32 <= 0);
    }

    #[test]
    fn no_screens_returns_none() {
        // Sin monitores disponibles → None (dejar al WM, no forzar geometría a ciegas).
        assert!(resolve_placement(None, &[], 1100, 760).is_none());
        let dh = hint(Some("x"), Some(0), Some(0), None, None);
        assert!(resolve_placement(Some(&dh), &[], 1100, 760).is_none());
    }

    #[test]
    fn target_screen_picks_first_when_no_primary_flag() {
        // Ningún monitor marcado primario → cae al primero (determinista).
        let s = vec![
            ScreenInfo {
                id: "a".into(),
                x: 0,
                y: 0,
                width: 800,
                height: 600,
                scale_factor: 1.0,
                is_primary: false,
            },
            ScreenInfo {
                id: "b".into(),
                x: 800,
                y: 0,
                width: 800,
                height: 600,
                scale_factor: 1.0,
                is_primary: false,
            },
        ];
        let t = target_screen(None, &s).unwrap();
        assert_eq!(t.id, "a");
    }
}
