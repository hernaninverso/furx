// services/audit_retention.rs — 019 T024 — retención + exportabilidad del audit del flujo review.
//
// Cierra el MUST de FR-005. DISEÑO (council 3-frontera, Opción B unánime): EXPORT-THEN-ROTATE, SIN
// DELETE físico. El audit vive en `events` (migración 001) + `review_audit_links` (migración 034),
// ambas APPEND-ONLY e INMUTABLES (triggers BEFORE UPDATE/DELETE → RAISE(ABORT)). Este módulo NO
// toca esos triggers ni borra filas: la retención = EXPORT a archivo (NDJSON/CSV, verificable por
// round-trip) + rotación a nivel archivo. La "purga" se registra como un evento NUEVO
// (`audit.rotation.completed`) y un manifest sellado — nunca como mutación del histórico.
//
// RECLAMACIÓN FÍSICA DE DISCO (deliberadamente FUERA de alcance, ver BACKLOG): una vez que el
// segmento fuera-de-ventana quedó exportado y SELLADO en un archivo verificable (con su sha256 en
// el manifest), reclamar el espacio de la DB sería rotar/compactar el ARCHIVO de la DB de forma
// externa (p.ej. exportar→archivar→re-crear), NUNCA `DELETE` de filas de `events`/`review_audit_links`
// (rompería la inmutabilidad y los triggers lo abortarían). `rotate_segment` NO borra nada: sólo
// exporta y sella el segmento, dejando el archivo como evidencia para esa rotación externa.

use crate::bases::audit::{AuditWriter, EventInput};
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

type Db = Arc<parking_lot::Mutex<Connection>>;

/// Política de retención. AMBOS límites son OPCIONALES y ortogonales:
///   - `max_age_days`: las filas más viejas que `now - max_age_days` quedan fuera de ventana.
///   - `max_events`: si hay más de `max_events` filas, el excedente MÁS VIEJO queda fuera de ventana.
/// "Fuera de ventana" = candidato a export+rotación; NUNCA se borra in-place.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RetentionPolicy {
    pub max_age_days: Option<u32>,
    pub max_events: Option<u64>,
}

impl RetentionPolicy {
    /// Timestamp de corte por edad (`datetime('now', '-N days')` en formato SQLite) si hay
    /// `max_age_days`. PURO: no toca DB. Devuelve `None` si la política no limita por edad.
    /// `now` es el instante de referencia (UTC, naive) — inyectable para tests deterministas.
    pub fn cutoff_ts(&self, now: chrono::NaiveDateTime) -> Option<String> {
        self.max_age_days.map(|days| {
            let cutoff = now - chrono::Duration::days(days as i64);
            // Mismo formato que `datetime('now')` de SQLite: "YYYY-MM-DD HH:MM:SS".
            cutoff.format("%Y-%m-%d %H:%M:%S").to_string()
        })
    }

    /// Selección PURA: dado el total de filas y sus timestamps ORDENADOS ascendente (más viejo
    /// primero), devuelve cuántas filas del comienzo caen FUERA de ventana según la política.
    /// Es la intersección de ambos límites: una fila cae fuera si es más vieja que el corte por
    /// edad O si está dentro del excedente por cantidad (la unión de ambos conjuntos del comienzo).
    /// `created_ats` debe venir en el MISMO orden cronológico ascendente que la tabla.
    pub fn out_of_window_count(&self, created_ats: &[String], now: chrono::NaiveDateTime) -> usize {
        let total = created_ats.len();
        // Por edad: cuántas filas del comienzo son < cutoff.
        let by_age = match self.cutoff_ts(now) {
            Some(cutoff) => created_ats.iter().take_while(|ts| **ts < cutoff).count(),
            None => 0,
        };
        // Por cantidad: el excedente por encima de `max_events` (las más viejas).
        let by_count = match self.max_events {
            Some(max) if (total as u64) > max => (total as u64 - max) as usize,
            _ => 0,
        };
        by_age.max(by_count)
    }
}

/// Alcance del export.
///   - `All` = todo el audit.
///   - `OutOfWindow{cutoff_ts}` = sólo las filas más viejas que el corte por EDAD (string SQLite).
///   - `OldestN{limit}` = las `limit` filas MÁS VIEJAS (orden determinista). Necesario para que la
///     rotación selle EXACTAMENTE el mismo conjunto fuera-de-ventana que `out_of_window_count`
///     cuando la política combina edad Y/O cantidad (MED-7), no sólo edad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportScope {
    All,
    OutOfWindow { cutoff_ts: String },
    OldestN { limit: u64 },
}

/// Formato serializado del export. NDJSON = una línea por fila (default verificable); CSV = mismo
/// contenido en columnas planas (segundo formato).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Ndjson,
    Csv,
}

impl ExportFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ExportFormat::Ndjson => "ndjson",
            ExportFormat::Csv => "csv",
        }
    }
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "ndjson" => Ok(ExportFormat::Ndjson),
            "csv" => Ok(ExportFormat::Csv),
            other => Err(anyhow!("formato de export desconocido: {other}")),
        }
    }
}

/// Una fila de audit exportable: el VÍNCULO (`review_audit_links`) + el timestamp del evento
/// inmutable (`events.at`). Preserva el vínculo audit↔change-set/hunk/approval COMPLETO para
/// trazabilidad. F-I/BYOK: estos campos son ids/estados/rationale de usuario — NUNCA secretos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditExportRow {
    pub event_id: String,
    pub action: String,
    pub group_id: Option<String>,
    pub hunk_id: Option<String>,
    pub approval_id: Option<String>,
    pub revision: Option<i64>,
    pub actor: String,
    pub target: String,
    pub rationale: String,
    /// `review_audit_links.created_at` (cuándo se selló el vínculo).
    pub created_at: String,
    /// `events.at` (cuándo se escribió el evento inmutable). `None` si el evento no se encontró
    /// (no debería pasar — el link nunca existe sin su evento — pero el LEFT JOIN lo tolera).
    pub event_at: Option<String>,
}

/// Manifest sellado de un export/rotación (lo que queda en `audit_export_manifests`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportManifest {
    pub manifest_id: String,
    pub kind: String,
    pub cutoff_ts: Option<String>,
    pub row_count: u64,
    pub content_sha256: String,
    pub path: String,
    pub format: String,
    /// HIGH-WATER MARK: `max(event_id)` del snapshot sellado (MED-5/TOCTOU). Como el snapshot se
    /// lee y se sella DENTRO de una única transacción y `event_id` es content-based (PK), este HWM
    /// prueba EXACTAMENTE qué conjunto cerró el manifest: cualquier evento concurrente posterior
    /// queda inequívocamente afuera. `None` sólo si el snapshot estaba vacío.
    pub high_water: Option<String>,
}

/// Recibo de una rotación: el manifest del export + el id del evento de rotación + cuántas filas
/// quedaron fuera de ventana (exportadas y selladas). NO reporta filas borradas: nada se borra.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RotationReceipt {
    pub manifest: ExportManifest,
    /// Id del evento NUEVO `audit.rotation.completed` (append-only).
    pub rotation_event_id: String,
    /// Filas fuera de ventana que se exportaron y sellaron (NO se borraron).
    pub sealed_rows: u64,
}

/// SELECT de las filas de audit según `scope`, ordenadas cronológicamente ascendente (orden
/// estable para que el hash del contenido sea determinista). LEFT JOIN con `events` por el
/// timestamp del evento inmutable. Read-only.
fn select_rows(conn: &Connection, scope: &ExportScope) -> Result<Vec<AuditExportRow>> {
    let base = "SELECT l.event_id, l.action, l.group_id, l.hunk_id, l.approval_id, l.revision, \
                       l.actor, l.target, l.rationale, l.created_at, e.at \
                FROM review_audit_links l LEFT JOIN events e ON e.id = l.event_id";
    let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<AuditExportRow> {
        Ok(AuditExportRow {
            event_id: r.get(0)?,
            action: r.get(1)?,
            group_id: r.get(2)?,
            hunk_id: r.get(3)?,
            approval_id: r.get(4)?,
            revision: r.get(5)?,
            actor: r.get(6)?,
            target: r.get(7)?,
            rationale: r.get(8)?,
            created_at: r.get(9)?,
            event_at: r.get(10)?,
        })
    };
    // MED-4: orden DETERMINISTA por una clave de contenido ESTABLE. `event_id` es content-based
    // (PK de `events`/`review_audit_links`), a diferencia de `rowid` que SQLite reasigna tras VACUUM
    // → desestabilizaba el sha256. `created_at ASC, event_id ASC` es estable bajo VACUUM/compactación.
    match scope {
        ExportScope::All => {
            let sql = format!("{base} ORDER BY l.created_at ASC, l.event_id ASC");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], map)?;
            rows.map(|r| r.map_err(Into::into)).collect()
        }
        ExportScope::OutOfWindow { cutoff_ts } => {
            let sql =
                format!("{base} WHERE l.created_at < ?1 ORDER BY l.created_at ASC, l.event_id ASC");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![cutoff_ts], map)?;
            rows.map(|r| r.map_err(Into::into)).collect()
        }
        ExportScope::OldestN { limit } => {
            // Las `limit` filas más viejas, en el MISMO orden determinista que el resto.
            let sql = format!("{base} ORDER BY l.created_at ASC, l.event_id ASC LIMIT ?1");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![*limit as i64], map)?;
            rows.map(|r| r.map_err(Into::into)).collect()
        }
    }
}

/// Serializa las filas al formato pedido. NDJSON: un objeto JSON por línea. CSV: header + una fila
/// por registro (mismas columnas, planas). El contenido es DETERMINISTA dado el orden de `rows`.
fn serialize_rows(rows: &[AuditExportRow], format: ExportFormat) -> Result<String> {
    match format {
        ExportFormat::Ndjson => {
            let mut out = String::new();
            for row in rows {
                out.push_str(&serde_json::to_string(row)?);
                out.push('\n');
            }
            Ok(out)
        }
        ExportFormat::Csv => {
            let mut out = String::new();
            out.push_str(
                "event_id,action,group_id,hunk_id,approval_id,revision,actor,target,rationale,created_at,event_at\n",
            );
            for row in rows {
                let cells = [
                    row.event_id.clone(),
                    row.action.clone(),
                    row.group_id.clone().unwrap_or_default(),
                    row.hunk_id.clone().unwrap_or_default(),
                    row.approval_id.clone().unwrap_or_default(),
                    row.revision.map(|r| r.to_string()).unwrap_or_default(),
                    row.actor.clone(),
                    row.target.clone(),
                    row.rationale.clone(),
                    row.created_at.clone(),
                    row.event_at.clone().unwrap_or_default(),
                ];
                let line = cells
                    .iter()
                    .map(|c| csv_escape(&csv_neutralize_formula(c)))
                    .collect::<Vec<_>>()
                    .join(",");
                out.push_str(&line);
                out.push('\n');
            }
            Ok(out)
        }
    }
}

/// Escape CSV mínimo (RFC 4180): comillas alrededor si hay coma/comilla/salto de línea; comillas
/// internas duplicadas. Suficiente para round-trip verificable por hash.
fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// HIGH-3: NEUTRALIZACIÓN de CSV formula injection. Si un campo user-controlled (rationale, actor,
/// target) EMPIEZA con un carácter que Excel/Sheets/LibreOffice interpretan como inicio de fórmula
/// (`=`, `+`, `-`, `@`) o con un carácter de control que el parser de la hoja puede usar para
/// adelantar el cursor de fórmula (TAB `\t`, CR `\r`), se prefija con un apóstrofo `'` ANTES del
/// quoting CSV. El apóstrofo fuerza a la hoja a tratar la celda como texto literal. Se aplica SIEMPRE
/// (no sólo a CSV-export): el costo es nulo y el round-trip por sha256 sigue siendo determinista.
fn csv_neutralize_formula(s: &str) -> String {
    match s.chars().next() {
        Some('=') | Some('+') | Some('-') | Some('@') | Some('\t') | Some('\r') => {
            format!("'{s}")
        }
        _ => s.to_string(),
    }
}

/// sha256 hex del contenido (para el manifest y el round-trip de verificación).
fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Inserta la fila del manifest en `audit_export_manifests` (append-only). `kind` = export|rotation.
#[allow(clippy::too_many_arguments)]
fn insert_manifest(
    conn: &Connection,
    manifest_id: &str,
    kind: &str,
    cutoff_ts: Option<&str>,
    row_count: u64,
    content_sha256: &str,
    path: &str,
    format: ExportFormat,
    high_water: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO audit_export_manifests \
         (manifest_id, kind, cutoff_ts, row_count, content_sha256, path, format, high_water) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            manifest_id,
            kind,
            cutoff_ts,
            row_count as i64,
            content_sha256,
            path,
            format.as_str(),
            high_water,
        ],
    )?;
    Ok(())
}

/// "HIGH-WATER" del snapshot ya leído (MED-5): `max(event_id)` de las filas seleccionadas. OJO: el
/// `event_id` es content-based (PK UUID/hash), NO un contador temporal — este valor NO es un
/// high-water TEMPORAL y NO prueba "lo posterior quedó afuera por orden de tiempo". Lo que SELLA el
/// snapshot exacto exportado es el par (este HWM + `content_sha256`): la evidencia PRIMARIA es el
/// hash del contenido; el HWM sólo etiqueta el conjunto cerrado de PKs. Si se necesitara un sello
/// temporal real, habría que agregar `max(events.at)` como dato ADICIONAL (no reemplaza al hash).
/// `None` si el snapshot está vacío.
fn high_water_of(rows: &[AuditExportRow]) -> Option<String> {
    rows.iter().map(|r| r.event_id.clone()).max()
}

/// HIGH-1: escribe el contenido a `path` SIN sobrescribir. Usa `create_new` (falla con `AlreadyExists`
/// si el archivo ya existe) → dos exports nunca se pisan, aun en el mismo segundo. La unicidad del
/// nombre la garantiza el caller (sufijo content-based + uuid); este guard es la red de seguridad.
fn write_new(path: &Path, content: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                anyhow!(
                    "el archivo de export ya existe (no se sobrescribe evidencia append-only): {}",
                    path.display()
                )
            } else {
                anyhow!("no se pudo crear el archivo de export {}: {e}", path.display())
            }
        })?;
    f.write_all(content)?;
    f.flush()?;
    Ok(())
}

/// CONFINAMIENTO pegado a la escritura (T024 audit, finding TOCTOU residual): re-canonicaliza el
/// `parent` REAL (resolviendo symlinks) JUSTO antes del open y reconfirma que cae dentro de
/// `base_canon`. Llamarlo INMEDIATAMENTE después de `create_dir_all(parent)` y ANTES de abrir el
/// archivo con `create_new`. Esto deja el check ADYACENTE al open: la única ventana de carrera que
/// queda es entre `canonicalize(parent)` y `open(create_new)`, y `create_new` ya falla si el destino
/// final existe (incluido si es un symlink). `base_canon` debe ser la base canónica permitida
/// (`audit_exports_dir()` canonicalizado); se canonicaliza acá de nuevo por robustez.
///
/// MODELO DE AMENAZA (honesto): Furx es una app LOCAL single-user que escribe en el HOME del propio
/// usuario. El atacante hipotético es un proceso local que reemplaza el dir destino por un symlink
/// fuera de la base en la micro-ventana entre el confirm previo (`confirm_within_base` en commands.rs)
/// y este open. La mitigación canonicalize-parent-justo-antes-de-open + `create_new` cierra el caso
/// PRÁCTICO (la ventana queda en microsegundos y `create_new` no sigue un symlink final existente).
/// Una garantía 100% libre de TOCTOU requeriría `openat`/`O_NOFOLLOW` con un dir-fd ya validado
/// (crate `nix`/`openat`), desproporcionado para una app local single-user — se deja anotado, NO se
/// implementa.
fn confirm_parent_within_base(base_canon: &Path, parent: &Path) -> Result<()> {
    let parent_canon = parent.canonicalize().map_err(|e| {
        anyhow!(
            "no se pudo canonicalizar el dir destino del export {}: {e}",
            parent.display()
        )
    })?;
    if !parent_canon.starts_with(base_canon) {
        return Err(anyhow!(
            "destino fuera del directorio permitido (symlink plantado pre-escritura): {}",
            parent_canon.display()
        ));
    }
    Ok(())
}

/// EXPORT VERIFICABLE: serializa el audit (según `scope`) al `format`, calcula su sha256, escribe el
/// archivo en `out_path` e inserta un manifest `kind=export`. NO borra nada. El round-trip
/// (re-leer el archivo → contar filas → re-hashear == manifest) es responsabilidad del verificador
/// (`verify_export`), que reusa exactamente este contenido.
///
/// `base_dir` es la BASE CONFINADA permitida (`~/.furx/audit-exports/` canónico). T024/TOCTOU:
/// JUSTO antes de abrir el archivo (tras `create_dir_all(parent)`), se re-canonicaliza el `parent`
/// real y se reconfirma `parent_canon.starts_with(base_canon)` — el confinamiento queda PEGADO a la
/// escritura efectiva, no sólo en el caller. Ver `confirm_parent_within_base` para el modelo de amenaza.
pub fn export_audit(
    db: &Db,
    scope: ExportScope,
    format: ExportFormat,
    out_path: &Path,
    base_dir: &Path,
) -> Result<ExportManifest> {
    let manifest_id = Uuid::new_v4().to_string();
    let cutoff = match &scope {
        ExportScope::All => None,
        ExportScope::OutOfWindow { cutoff_ts } => Some(cutoff_ts.clone()),
        // OldestN no tiene cutoff por edad propio; el corte informativo lo sella el caller (rotación).
        ExportScope::OldestN { .. } => None,
    };

    // MED-5 (TOCTOU snapshot↔manifest): leer las filas y calcular HWM/hash dentro de UNA transacción
    // IMMEDIATE → lectura consistente, sin que un evento concurrente se cuele en medio del snapshot.
    // El SELLO del snapshot exacto es (HWM + `content_sha256`): el hash del contenido es la evidencia
    // PRIMARIA de QUÉ se exportó; el HWM (`max(event_id)`, content-based, NO temporal) sólo etiqueta
    // el conjunto cerrado de PKs. NO es prueba de posterioridad por orden de tiempo. Read-only.
    let (content, row_count, high_water) = {
        let mut conn = db.lock();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let rows = select_rows(&tx, &scope)?;
        let content = serialize_rows(&rows, format)?;
        let row_count = rows.len() as u64;
        let high_water = high_water_of(&rows);
        // Read-only: cerrar sin commit pendiente (rollback de una tx sin writes es no-op).
        tx.commit()?;
        (content, row_count, high_water)
    };
    let content_sha256 = sha256_hex(&content);

    // Escribir el archivo ANTES de sellar el manifest: si la escritura falla, no queda un manifest
    // apuntando a un archivo inexistente. HIGH-1: nombre ÚNICO (lo garantiza el caller con un sufijo
    // content-based/uuid) + `create_new` → dos exports en el mismo segundo NUNCA se pisan; un manifest
    // append-only jamás apunta a evidencia REEMPLAZADA.
    //
    // T024/TOCTOU residual: el confinamiento se PEGA a la escritura efectiva. Canonicalizamos la base
    // permitida una vez, y tras `create_dir_all(parent)` re-canonicalizamos el `parent` real JUSTO
    // antes del open y reconfirmamos que cae dentro de la base. Así el límite lo conoce la función que
    // ESCRIBE, no sólo el caller (`confirm_within_base` en commands.rs sigue como defensa en capas).
    let base_canon = base_dir.canonicalize().map_err(|e| {
        anyhow!(
            "no se pudo canonicalizar la base de audit-exports {}: {e}",
            base_dir.display()
        )
    })?;
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
            // Revalidación PEGADA al open: el parent real (resolviendo symlinks) debe seguir dentro
            // de la base. Si un symlink se plantó entre el confirm del caller y este punto, abortamos
            // SIN escribir.
            confirm_parent_within_base(&base_canon, parent)?;
        } else {
            // `out_path` sin parent explícito (nombre suelto): el cwd debe estar dentro de la base.
            confirm_parent_within_base(&base_canon, std::path::Path::new("."))?;
        }
    }
    write_new(out_path, content.as_bytes())?;

    let path_str = out_path.to_string_lossy().to_string();
    {
        let conn = db.lock();
        insert_manifest(
            &conn,
            &manifest_id,
            "export",
            cutoff.as_deref(),
            row_count,
            &content_sha256,
            &path_str,
            format,
            high_water.as_deref(),
        )?;
    }

    Ok(ExportManifest {
        manifest_id,
        kind: "export".into(),
        cutoff_ts: cutoff,
        row_count,
        content_sha256,
        path: path_str,
        format: format.as_str().into(),
        high_water,
    })
}

/// Verifica un export por ROUND-TRIP REAL: re-lee el archivo, re-hashea y CUENTA REGISTROS PARSEANDO
/// el formato (no contando líneas crudas). Devuelve `true` si el hash coincide con el del manifest Y
/// el conteo PARSEADO coincide. MED-6: un campo `rationale` con saltos de línea entre comillas (CSV)
/// o cualquier multilínea ya NO infla el conteo — se cuenta según el grammar del formato, lo que
/// además valida que el archivo es PARSEABLE de verdad (un CSV mal formado / NDJSON con JSON inválido
/// hace que `count_records` devuelva Err → la verificación falla).
pub fn verify_export(manifest: &ExportManifest, path: &Path) -> Result<bool> {
    let content = std::fs::read_to_string(path)?;
    let rehash = sha256_hex(&content);
    let format = ExportFormat::parse(&manifest.format)?;
    let n = count_records(&content, format)?;
    Ok(rehash == manifest.content_sha256 && n == manifest.row_count)
}

/// Cuenta REGISTROS parseando el formato (no líneas crudas). MED-6.
///   - NDJSON: cada línea no vacía debe ser un objeto JSON válido (sino Err).
///   - CSV: parser RFC-4180 que respeta quoting y newlines DENTRO de comillas → registros = nº de
///     filas físicas (separadas por newlines no-quoted) menos el header. Valida quoting balanceado.
fn count_records(content: &str, format: ExportFormat) -> Result<u64> {
    match format {
        ExportFormat::Ndjson => {
            let mut n = 0u64;
            for (i, line) in content.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                serde_json::from_str::<serde_json::Value>(line)
                    .map_err(|e| anyhow!("NDJSON inválido en línea {}: {e}", i + 1))?;
                n += 1;
            }
            Ok(n)
        }
        ExportFormat::Csv => {
            let physical = csv_record_count(content)?;
            // El header SIEMPRE está presente (lo emite `serialize_rows`), aun con 0 filas.
            Ok(physical.saturating_sub(1))
        }
    }
}

/// Cuenta filas físicas CSV (RFC-4180) respetando quoting: un newline DENTRO de comillas NO termina
/// la fila; `""` es una comilla escapada. Devuelve Err si el quoting queda abierto al EOF (CSV roto).
fn csv_record_count(content: &str) -> Result<u64> {
    let mut rows: u64 = 0;
    let mut in_quotes = false;
    let mut row_has_content = false;
    let mut chars = content.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_quotes {
                    // `""` dentro de comillas = comilla literal escapada.
                    if chars.peek() == Some(&'"') {
                        chars.next();
                    } else {
                        in_quotes = false;
                    }
                } else {
                    in_quotes = true;
                }
                row_has_content = true;
            }
            '\n' if !in_quotes => {
                if row_has_content {
                    rows += 1;
                }
                row_has_content = false;
            }
            '\r' if !in_quotes => { /* ignorar CR fuera de comillas (CRLF) */ }
            // Cualquier otro char (fuera o dentro de comillas) cuenta como contenido de la fila.
            _ => {
                row_has_content = true;
            }
        }
    }
    if in_quotes {
        return Err(anyhow!("CSV mal formado: comillas sin cerrar al fin del archivo"));
    }
    // Última fila sin newline final.
    if row_has_content {
        rows += 1;
    }
    Ok(rows)
}

/// ROTACIÓN del segmento fuera-de-ventana (export-then-rotate, SIN DELETE):
///   (a) resuelve por `policy` (`now`) el MISMO conjunto fuera-de-ventana que `out_of_window_count`
///       (edad Y/O cantidad — MED-7) y lo exporta a `out_dir` con scope=OldestN(n),
///   (b) registra un evento NUEVO `audit.rotation.completed` (append-only, vía `AuditWriter`)
///       referenciando el `manifest_id` + `content_sha256` + `high_water`,
///   (c) inserta un manifest `kind=rotation` (con su HWM).
/// NO borra filas de `events`/`review_audit_links` — la reclamación física de disco es rotación de
/// archivo externa (ver doc-comment de cabecera + BACKLOG). El receipt reporta cuántas filas
/// quedaron fuera de ventana (exportadas y selladas), coherente con `retention_status.out_of_window`.
///
/// La política DEBE limitar por edad y/o por cantidad; si NO define ninguno (`max_age_days=None` y
/// `max_events=None`), no hay segmento por rotar → Err (usar `export_audit(All)` para un dump completo).
///
/// `base_dir` es la BASE CONFINADA permitida; se propaga a `export_audit` para que la revalidación
/// del confinamiento ocurra PEGADA a la escritura (T024/TOCTOU). `out_dir` debe ser `base_dir` o un
/// subdir confinado dentro de ella (el caller lo garantiza con `confined_subpath`).
pub fn rotate_segment(
    db: &Db,
    audit: &AuditWriter,
    policy: &RetentionPolicy,
    now: chrono::NaiveDateTime,
    out_dir: &Path,
    base_dir: &Path,
) -> Result<RotationReceipt> {
    if policy.max_age_days.is_none() && policy.max_events.is_none() {
        return Err(anyhow!(
            "la política no define ni max_age_days ni max_events — no hay segmento por rotar"
        ));
    }

    // MED-7: el conjunto a rotar es EXACTAMENTE el fuera-de-ventana de la política (edad O cantidad),
    // calculado igual que `retention_status.out_of_window`. Leemos los created_at ordenados y
    // delegamos en `out_of_window_count`, luego sellamos las N MÁS VIEJAS con scope=OldestN.
    let n = {
        let conn = db.lock();
        let created_ats: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT created_at FROM review_audit_links ORDER BY created_at ASC, event_id ASC",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        policy.out_of_window_count(&created_ats, now) as u64
    };

    // `cutoff` por edad para registrar en el manifest/evento (informativo; el conjunto real es OldestN).
    let cutoff = policy.cutoff_ts(now);

    // HIGH-1: nombre ÚNICO (timestamp + uuid corto) → dos rotaciones en el mismo segundo no colisionan.
    let out_path = out_dir.join(format!(
        "audit-rotation-{}-{}.ndjson",
        now.format("%Y%m%dT%H%M%S"),
        Uuid::new_v4().simple()
    ));

    // (a) export del segmento fuera-de-ventana (kind=export, manifest propio). OldestN sella EXACTO
    // las N filas más viejas que `out_of_window_count` identificó.
    let export = export_audit(
        db,
        ExportScope::OldestN { limit: n },
        ExportFormat::Ndjson,
        &out_path,
        base_dir,
    )?;
    let sealed_rows = export.row_count;

    // (b) evento NUEVO append-only que SELLA la rotación (referencia manifest + hash). NO muta nada.
    let rotation_event_id = audit.write(EventInput {
        kind: "audit.rotation.completed",
        actor: "system:retention",
        pane_id: None,
        card_id: None,
        correlation_id: Some(export.manifest_id.as_str()),
        payload: serde_json::json!({
            "manifest_id": export.manifest_id,
            "content_sha256": export.content_sha256,
            "cutoff_ts": cutoff,
            "sealed_rows": sealed_rows,
            "path": export.path,
            "note": "export-then-rotate: filas selladas en archivo, NO borradas de la DB",
        }),
    })?;

    // (c) manifest kind=rotation (apunta al MISMO archivo + hash + HWM del segmento exportado).
    let rotation_manifest_id = Uuid::new_v4().to_string();
    {
        let conn = db.lock();
        insert_manifest(
            &conn,
            &rotation_manifest_id,
            "rotation",
            cutoff.as_deref(),
            sealed_rows,
            &export.content_sha256,
            &export.path,
            ExportFormat::Ndjson,
            export.high_water.as_deref(),
        )?;
    }

    Ok(RotationReceipt {
        manifest: ExportManifest {
            manifest_id: rotation_manifest_id,
            kind: "rotation".into(),
            cutoff_ts: cutoff,
            row_count: sealed_rows,
            content_sha256: export.content_sha256,
            path: export.path,
            format: ExportFormat::Ndjson.as_str().into(),
            high_water: export.high_water,
        },
        rotation_event_id,
        sealed_rows,
    })
}

/// Estado de retención: política activa + nº de filas fuera de ventana + último manifest. Read-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetentionStatus {
    pub policy: RetentionPolicy,
    pub total_rows: u64,
    pub out_of_window: u64,
    pub last_manifest: Option<ExportManifest>,
}

/// Calcula el estado de retención para una política dada y un `now` de referencia. Read-only.
pub fn retention_status(
    db: &Db,
    policy: &RetentionPolicy,
    now: chrono::NaiveDateTime,
) -> Result<RetentionStatus> {
    let conn = db.lock();
    // Todos los created_at ordenados ascendente para el cálculo de fuera-de-ventana.
    // MED-4: orden determinista por event_id (estable bajo VACUUM), no rowid.
    let created_ats: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT created_at FROM review_audit_links ORDER BY created_at ASC, event_id ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let total_rows = created_ats.len() as u64;
    let out_of_window = policy.out_of_window_count(&created_ats, now) as u64;

    // Último manifest (por created_at, luego rowid). MED-8: distinguir "no hay manifests todavía"
    // (QueryReturnedNoRows → None legítimo) de un error REAL (tabla faltante/corrupción → Err
    // propagado), en vez de colapsar TODO a None con `.ok()` y enmascarar una DB rota.
    let last_manifest = match conn.query_row(
        "SELECT manifest_id, kind, cutoff_ts, row_count, content_sha256, path, format, high_water \
             FROM audit_export_manifests ORDER BY created_at DESC, rowid DESC LIMIT 1",
        [],
        |r| {
            Ok(ExportManifest {
                manifest_id: r.get(0)?,
                kind: r.get(1)?,
                cutoff_ts: r.get(2)?,
                row_count: r.get::<_, i64>(3)? as u64,
                content_sha256: r.get(4)?,
                path: r.get(5)?,
                format: r.get(6)?,
                high_water: r.get(7)?,
            })
        },
    ) {
        Ok(m) => Some(m),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(e) => return Err(e.into()),
    };

    Ok(RetentionStatus {
        policy: policy.clone(),
        total_rows,
        out_of_window,
        last_manifest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::review_audit::{record, ReviewAction, ReviewAuditEntry, ReviewTargetLink};

    fn test_db() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        // events + triggers (001), vínculo de audit (034), manifests de retención (038).
        conn.execute_batch(include_str!("../../migrations/001_init.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/034_review_audit_link.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/038_audit_retention.sql"))
            .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    fn now() -> chrono::NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, 6, 1)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    /// Base confinada para los tests: los archivos/dirs de export se crean bajo `temp_dir()`, así que
    /// la base canónica permitida es `temp_dir()` mismo. Se canonicaliza por robustez (en macOS
    /// `/tmp` es symlink a `/private/tmp`).
    fn test_base() -> std::path::PathBuf {
        std::env::temp_dir()
    }

    /// Inserta una fila de audit con un `created_at` explícito (bypassa el DEFAULT para tener
    /// timestamps deterministas en los tests de ventana). Inserta el evento a mano para mantener
    /// el invariante link↔evento. Devuelve el event_id.
    fn seed_at(db: &Db, action: &str, group_id: &str, hunk_id: &str, created_at: &str) -> String {
        let id = Uuid::new_v4().to_string();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO events (id, at, kind, actor, payload) VALUES (?1, ?2, ?3, 'user:t', '{}')",
            params![id, created_at, format!("review.{action}")],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO review_audit_links \
             (event_id, action, group_id, hunk_id, actor, target, rationale, created_at) \
             VALUES (?1,?2,?3,?4,'user:t','tgt','why',?5)",
            params![id, action, group_id, hunk_id, created_at],
        )
        .unwrap();
        id
    }

    #[test]
    fn cutoff_ts_pure_by_age() {
        // max_age_days=30 sobre now=2026-06-01 12:00:00 → corte 2026-05-02 12:00:00.
        let p = RetentionPolicy {
            max_age_days: Some(30),
            max_events: None,
        };
        assert_eq!(p.cutoff_ts(now()).as_deref(), Some("2026-05-02 12:00:00"));
        // Sin max_age_days → no hay corte por edad.
        let p2 = RetentionPolicy {
            max_age_days: None,
            max_events: Some(10),
        };
        assert_eq!(p2.cutoff_ts(now()), None);
    }

    #[test]
    fn out_of_window_pure_by_age_and_count() {
        // 5 filas: 3 viejas (< corte) + 2 nuevas.
        let ats = vec![
            "2026-01-01 00:00:00".to_string(),
            "2026-02-01 00:00:00".to_string(),
            "2026-03-01 00:00:00".to_string(),
            "2026-05-30 00:00:00".to_string(),
            "2026-05-31 00:00:00".to_string(),
        ];
        // Sólo por edad (corte 2026-05-02): 3 fuera.
        let by_age = RetentionPolicy {
            max_age_days: Some(30),
            max_events: None,
        };
        assert_eq!(by_age.out_of_window_count(&ats, now()), 3);
        // Sólo por cantidad (max 2): 3 excedentes más viejos.
        let by_count = RetentionPolicy {
            max_age_days: None,
            max_events: Some(2),
        };
        assert_eq!(by_count.out_of_window_count(&ats, now()), 3);
        // Combinada: el MAYOR de ambos (age=3, count(max4)=1 → 3).
        let both = RetentionPolicy {
            max_age_days: Some(30),
            max_events: Some(4),
        };
        assert_eq!(both.out_of_window_count(&ats, now()), 3);
        // Política vacía: nada fuera de ventana.
        assert_eq!(RetentionPolicy::default().out_of_window_count(&ats, now()), 0);
    }

    #[test]
    fn export_round_trip_count_and_hash_match_manifest() {
        let db = test_db();
        seed_at(&db, "compare", "g1", "h1", "2026-05-30 10:00:00");
        seed_at(&db, "approve", "g1", "h1", "2026-05-30 10:01:00");
        seed_at(&db, "apply", "g1", "h1", "2026-05-30 10:02:00");

        let tmp = std::env::temp_dir().join(format!("furx-audit-export-{}.ndjson", std::process::id()));
        let manifest =
            export_audit(&db, ExportScope::All, ExportFormat::Ndjson, &tmp, &test_base()).unwrap();
        assert_eq!(manifest.row_count, 3);
        // round-trip: re-leer archivo, re-contar, re-hashear == manifest.
        assert!(verify_export(&manifest, &tmp).unwrap(), "round-trip debe verificar");
        // Y a mano: el archivo tiene 3 líneas.
        let content = std::fs::read_to_string(&tmp).unwrap();
        assert_eq!(content.lines().filter(|l| !l.is_empty()).count(), 3);
        assert_eq!(sha256_hex(&content), manifest.content_sha256);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn export_csv_round_trip() {
        let db = test_db();
        seed_at(&db, "reject", "g9", "h9", "2026-05-30 10:00:00");
        let tmp = std::env::temp_dir().join(format!("furx-audit-export-{}.csv", std::process::id()));
        let manifest =
            export_audit(&db, ExportScope::All, ExportFormat::Csv, &tmp, &test_base()).unwrap();
        assert_eq!(manifest.format, "csv");
        assert_eq!(manifest.row_count, 1);
        assert!(verify_export(&manifest, &tmp).unwrap());
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn ndjson_line_preserves_change_set_link() {
        // Un decide (approve) trae group_id + hunk_id en la línea NDJSON (vínculo preservado).
        let db = test_db();
        let audit = AuditWriter::new(db.clone());
        record(
            &db,
            &audit,
            ReviewAuditEntry {
                action: ReviewAction::Approve,
                actor: "user:test",
                target: "g7/h7",
                rationale: "ok",
                link: ReviewTargetLink {
                    group_id: Some("g7".into()),
                    hunk_id: Some("h7".into()),
                    approval_id: Some("a7".into()),
                    revision: Some(2),
                },
            },
        )
        .unwrap();
        let tmp = std::env::temp_dir().join(format!("furx-audit-link-{}.ndjson", std::process::id()));
        export_audit(&db, ExportScope::All, ExportFormat::Ndjson, &tmp, &test_base()).unwrap();
        let content = std::fs::read_to_string(&tmp).unwrap();
        let line = content.lines().next().unwrap();
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["group_id"], "g7");
        assert_eq!(v["hunk_id"], "h7");
        assert_eq!(v["approval_id"], "a7");
        assert_eq!(v["revision"], 2);
        assert_eq!(v["action"], "approve");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn manifest_table_is_append_only() {
        let db = test_db();
        let tmp = std::env::temp_dir().join(format!("furx-audit-mtbl-{}.ndjson", std::process::id()));
        let m = export_audit(&db, ExportScope::All, ExportFormat::Ndjson, &tmp, &test_base()).unwrap();
        let conn = db.lock();
        let upd = conn.execute(
            "UPDATE audit_export_manifests SET row_count = 999 WHERE manifest_id = ?1",
            params![m.manifest_id],
        );
        assert!(upd.is_err(), "UPDATE del manifest debió abortar (append-only)");
        let del = conn.execute(
            "DELETE FROM audit_export_manifests WHERE manifest_id = ?1",
            params![m.manifest_id],
        );
        assert!(del.is_err(), "DELETE del manifest debió abortar (append-only)");
        drop(conn);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn rotation_seals_event_and_manifest_without_losing_rows() {
        let db = test_db();
        let audit = AuditWriter::new(db.clone());
        // 2 viejas (fuera de ventana) + 2 nuevas (dentro).
        seed_at(&db, "compare", "g1", "h1", "2026-01-01 00:00:00");
        seed_at(&db, "approve", "g1", "h1", "2026-02-01 00:00:00");
        seed_at(&db, "compare", "g2", "h2", "2026-05-31 00:00:00");
        seed_at(&db, "approve", "g2", "h2", "2026-05-31 12:00:00");

        let count_links = |db: &Db| -> i64 {
            db.lock()
                .query_row("SELECT COUNT(*) FROM review_audit_links", [], |r| r.get(0))
                .unwrap()
        };
        let count_events = |db: &Db| -> i64 {
            db.lock()
                .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
                .unwrap()
        };
        let links_before = count_links(&db);
        let events_before = count_events(&db);

        let policy = RetentionPolicy {
            max_age_days: Some(30),
            max_events: None,
        };
        let out_dir = std::env::temp_dir().join(format!("furx-rot-{}", std::process::id()));
        let receipt = rotate_segment(&db, &audit, &policy, now(), &out_dir, &test_base()).unwrap();

        // 2 filas fuera de ventana selladas.
        assert_eq!(receipt.sealed_rows, 2);
        assert_eq!(receipt.manifest.kind, "rotation");
        assert_eq!(receipt.manifest.row_count, 2);
        // El evento de rotación NUEVO existe en events.
        let rot_exists: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM events WHERE id = ?1 AND kind = 'audit.rotation.completed'",
                params![receipt.rotation_event_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rot_exists, 1);
        // El manifest kind=rotation está sellado.
        let rot_manifests: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM audit_export_manifests WHERE kind = 'rotation'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rot_manifests, 1);

        // NINGUNA fila de audit se perdió: review_audit_links igual, events SOLO creció (por el
        // evento de rotación), nunca decreció.
        assert_eq!(count_links(&db), links_before, "no se borró ningún link");
        assert_eq!(
            count_events(&db),
            events_before + 1,
            "events sólo creció por el evento de rotación"
        );

        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn in_window_rows_intact_after_rotate() {
        let db = test_db();
        let audit = AuditWriter::new(db.clone());
        seed_at(&db, "compare", "gold", "hold", "2026-01-01 00:00:00"); // fuera
        seed_at(&db, "approve", "gnew", "hnew", "2026-05-31 00:00:00"); // dentro

        let policy = RetentionPolicy {
            max_age_days: Some(30),
            max_events: None,
        };
        let out_dir = std::env::temp_dir().join(format!("furx-rot2-{}", std::process::id()));
        rotate_segment(&db, &audit, &policy, now(), &out_dir, &test_base()).unwrap();

        // La fila dentro de ventana sigue consultable sin tocar.
        let in_window: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM review_audit_links WHERE group_id = 'gnew'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(in_window, 1, "la fila dentro de ventana queda intacta");
        // Y la fila fuera de ventana TAMBIÉN sigue en la DB (no se borra; sólo se selló en archivo).
        let out_window: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM review_audit_links WHERE group_id = 'gold'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(out_window, 1, "la fila fuera de ventana NO se borra (export-then-rotate)");

        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn retention_status_reports_policy_and_out_of_window() {
        let db = test_db();
        seed_at(&db, "compare", "g1", "h1", "2026-01-01 00:00:00"); // fuera
        seed_at(&db, "approve", "g2", "h2", "2026-05-31 00:00:00"); // dentro
        let policy = RetentionPolicy {
            max_age_days: Some(30),
            max_events: None,
        };
        let st = retention_status(&db, &policy, now()).unwrap();
        assert_eq!(st.total_rows, 2);
        assert_eq!(st.out_of_window, 1);
        assert_eq!(st.policy, policy);
        assert!(st.last_manifest.is_none(), "sin export todavía → sin manifest");
    }

    /// Inserta una fila de audit con actor/target/rationale EXPLÍCITOS (para los tests de CSV
    /// formula-injection y secret-leak). Devuelve el event_id.
    fn seed_fields(
        db: &Db,
        action: &str,
        actor: &str,
        target: &str,
        rationale: &str,
        created_at: &str,
    ) -> String {
        let id = Uuid::new_v4().to_string();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO events (id, at, kind, actor, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                created_at,
                format!("review.{action}"),
                actor,
                // payload con un "secreto" plantado: el export NUNCA debe incluirlo.
                r#"{"secret":"sk-LIVE-must-never-leak","token":"BYOK-key-1234"}"#
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO review_audit_links \
             (event_id, action, group_id, hunk_id, actor, target, rationale, created_at) \
             VALUES (?1,?2,'g','h',?3,?4,?5,?6)",
            params![id, action, actor, target, rationale, created_at],
        )
        .unwrap();
        id
    }

    // ── HIGH-1: filename único + no-overwrite ──
    #[test]
    fn export_create_new_refuses_overwrite() {
        let db = test_db();
        seed_at(&db, "compare", "g1", "h1", "2026-05-30 10:00:00");
        let tmp =
            std::env::temp_dir().join(format!("furx-overwrite-{}.ndjson", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        export_audit(&db, ExportScope::All, ExportFormat::Ndjson, &tmp, &test_base()).unwrap();
        // Segundo export al MISMO path debe FALLAR (create_new), nunca pisar la evidencia.
        let err =
            export_audit(&db, ExportScope::All, ExportFormat::Ndjson, &tmp, &test_base()).unwrap_err();
        assert!(
            err.to_string().contains("ya existe"),
            "el 2º export al mismo path debe rechazarse: {err}"
        );
        let _ = std::fs::remove_file(&tmp);
    }

    // ── HIGH-3: CSV formula injection neutralizada ──
    #[test]
    fn csv_formula_injection_is_neutralized() {
        // Caracteres peligrosos al inicio: =, +, -, @, TAB, CR.
        assert_eq!(csv_neutralize_formula("=1+1"), "'=1+1");
        assert_eq!(csv_neutralize_formula("+cmd"), "'+cmd");
        assert_eq!(csv_neutralize_formula("-2"), "'-2");
        assert_eq!(csv_neutralize_formula("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(csv_neutralize_formula("\tx"), "'\tx");
        assert_eq!(csv_neutralize_formula("\rx"), "'\rx");
        // Texto benigno NO se toca.
        assert_eq!(csv_neutralize_formula("ok rationale"), "ok rationale");
        assert_eq!(csv_neutralize_formula("user:test"), "user:test");

        // End-to-end: un rationale con `=HYPERLINK(...)` sale neutralizado en el CSV.
        let db = test_db();
        seed_fields(
            &db,
            "approve",
            "user:h",
            "tgt",
            "=HYPERLINK(\"http://evil\",\"x\")",
            "2026-05-30 10:00:00",
        );
        let tmp = std::env::temp_dir().join(format!("furx-csvinj-{}.csv", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        export_audit(&db, ExportScope::All, ExportFormat::Csv, &tmp, &test_base()).unwrap();
        let content = std::fs::read_to_string(&tmp).unwrap();
        // El campo quedó prefijado con `'` y luego quoteado (por el `=`+comillas internas).
        assert!(
            content.contains("'=HYPERLINK"),
            "el rationale-fórmula debe estar neutralizado con apóstrofo: {content}"
        );
        assert!(
            !content.contains(",=HYPERLINK"),
            "no debe quedar un `=` arrancando una celda sin neutralizar"
        );
        let _ = std::fs::remove_file(&tmp);
    }

    // ── MED-4: orden determinista por event_id (no rowid) ──
    #[test]
    fn export_order_stable_by_event_id() {
        // Dos filas con el MISMO created_at: el tie-break por event_id debe ser determinista.
        let db = test_db();
        let id_z = {
            // forzar event_ids conocidos: insertamos a mano con ids ordenables.
            let conn = db.lock();
            for (eid, act) in [("aaaa", "compare"), ("zzzz", "approve")] {
                conn.execute(
                    "INSERT INTO events (id, at, kind, actor, payload) VALUES (?1,'2026-05-30 10:00:00',?2,'user:t','{}')",
                    params![eid, format!("review.{act}")],
                ).unwrap();
                conn.execute(
                    "INSERT INTO review_audit_links (event_id, action, group_id, hunk_id, actor, target, rationale, created_at) \
                     VALUES (?1,?2,'g','h','user:t','t','r','2026-05-30 10:00:00')",
                    params![eid, act],
                ).unwrap();
            }
            "zzzz"
        };
        let conn = db.lock();
        let rows = select_rows(&conn, &ExportScope::All).unwrap();
        drop(conn);
        assert_eq!(rows.len(), 2);
        // Orden ascendente por event_id con created_at empatado: aaaa antes que zzzz.
        assert_eq!(rows[0].event_id, "aaaa");
        assert_eq!(rows[1].event_id, id_z);
    }

    // ── MED-5: high-water mark en el manifest ──
    #[test]
    fn manifest_seals_high_water_mark() {
        let db = test_db();
        let id1 = seed_at(&db, "compare", "g1", "h1", "2026-05-30 10:00:00");
        let id2 = seed_at(&db, "approve", "g1", "h1", "2026-05-30 10:01:00");
        let expected_hwm = std::cmp::max(id1, id2);
        let tmp = std::env::temp_dir().join(format!("furx-hwm-{}.ndjson", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let m = export_audit(&db, ExportScope::All, ExportFormat::Ndjson, &tmp, &test_base()).unwrap();
        assert_eq!(
            m.high_water.as_deref(),
            Some(expected_hwm.as_str()),
            "el manifest debe sellar max(event_id) como HWM"
        );
        // Persistido en la tabla.
        let stored: Option<String> = db
            .lock()
            .query_row(
                "SELECT high_water FROM audit_export_manifests WHERE manifest_id = ?1",
                params![m.manifest_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, m.high_water);
        let _ = std::fs::remove_file(&tmp);
    }

    // ── MED-6: verify CSV real (round-trip parseable, no conteo de líneas) ──
    #[test]
    fn verify_csv_handles_multiline_rationale() {
        let db = test_db();
        // rationale con newline embebido → en CSV va quoteado y ocupa 2 líneas físicas.
        seed_fields(
            &db,
            "reject",
            "user:h",
            "tgt",
            "line1\nline2 con, coma",
            "2026-05-30 10:00:00",
        );
        let tmp = std::env::temp_dir().join(format!("furx-multiline-{}.csv", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let m = export_audit(&db, ExportScope::All, ExportFormat::Csv, &tmp, &test_base()).unwrap();
        assert_eq!(m.row_count, 1);
        // El archivo tiene MÁS de 2 líneas físicas (header + rationale multilínea), pero verify
        // debe contar 1 registro real parseando el quoting.
        let content = std::fs::read_to_string(&tmp).unwrap();
        assert!(content.lines().count() >= 3, "debe haber >2 líneas físicas por el newline embebido");
        assert!(verify_export(&m, &tmp).unwrap(), "verify debe parsear el CSV y contar 1 registro");
        assert_eq!(count_records(&content, ExportFormat::Csv).unwrap(), 1);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn count_records_rejects_malformed_csv() {
        // CSV con comilla sin cerrar → Err (no parseable).
        let bad = "header\n\"abc";
        assert!(count_records(bad, ExportFormat::Csv).is_err());
        // NDJSON con JSON inválido → Err.
        let bad_nd = "{not json}";
        assert!(count_records(bad_nd, ExportFormat::Ndjson).is_err());
    }

    // ── MED-7: rotate honra max_events (no sólo max_age_days) ──
    #[test]
    fn rotate_honors_max_events_policy() {
        let db = test_db();
        let audit = AuditWriter::new(db.clone());
        // 5 filas TODAS recientes (dentro de cualquier ventana por edad).
        for i in 0..5 {
            seed_at(&db, "compare", "g", "h", &format!("2026-05-31 10:0{i}:00"));
        }
        // Política SÓLO por cantidad: max_events=2 → 3 excedentes (las más viejas) fuera de ventana.
        let policy = RetentionPolicy {
            max_age_days: None,
            max_events: Some(2),
        };
        // out_of_window_count coincide con lo que rota.
        let st = retention_status(&db, &policy, now()).unwrap();
        assert_eq!(st.out_of_window, 3);
        let out_dir = std::env::temp_dir().join(format!("furx-rotmax-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out_dir);
        let receipt = rotate_segment(&db, &audit, &policy, now(), &out_dir, &test_base()).unwrap();
        assert_eq!(receipt.sealed_rows, 3, "rotación debe sellar las 3 fuera-de-ventana por cantidad");
        assert_eq!(receipt.sealed_rows, st.out_of_window);
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn rotate_empty_policy_errs() {
        let db = test_db();
        let audit = AuditWriter::new(db.clone());
        let policy = RetentionPolicy::default(); // ni age ni count
        let out_dir = std::env::temp_dir().join(format!("furx-rotempty-{}", std::process::id()));
        let err = rotate_segment(&db, &audit, &policy, now(), &out_dir, &test_base()).unwrap_err();
        assert!(err.to_string().contains("no hay segmento por rotar"));
    }

    // ── MED-8: retention_status distingue no-rows de error real ──
    #[test]
    fn retention_status_propagates_real_error_not_none() {
        // DB SIN la tabla de manifests (migración 038 ausente) → query_row da un error REAL,
        // que debe PROPAGARSE, no colapsar a last_manifest=None.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/001_init.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/034_review_audit_link.sql"))
            .unwrap();
        // NO 038 a propósito.
        let db = Arc::new(parking_lot::Mutex::new(conn));
        let policy = RetentionPolicy {
            max_age_days: Some(30),
            max_events: None,
        };
        let res = retention_status(&db, &policy, now());
        assert!(res.is_err(), "tabla de manifests ausente debe propagar Err, no None");
    }

    // ── secret-leak: el export NUNCA incluye payload crudo / material BYOK ──
    #[test]
    fn export_never_leaks_raw_payload_or_secrets() {
        let db = test_db();
        // El payload del evento tiene un "secreto" plantado (ver seed_fields).
        seed_fields(&db, "approve", "user:h", "tgt", "rationale ok", "2026-05-30 10:00:00");
        let tmp = std::env::temp_dir().join(format!("furx-leak-{}.ndjson", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        export_audit(&db, ExportScope::All, ExportFormat::Ndjson, &tmp, &test_base()).unwrap();
        let content = std::fs::read_to_string(&tmp).unwrap();
        assert!(
            !content.contains("sk-LIVE-must-never-leak"),
            "el export NO debe contener el secreto del payload crudo"
        );
        assert!(
            !content.contains("BYOK-key-1234"),
            "el export NO debe contener material BYOK del payload"
        );
        assert!(
            !content.contains("\"payload\""),
            "el export NO debe incluir la columna payload"
        );
        // Sanity: SÍ contiene los campos estructurados esperados.
        let line = content.lines().next().unwrap();
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["action"], "approve");
        assert_eq!(v["rationale"], "rationale ok");
        assert!(v.get("payload").is_none(), "ni siquiera una clave payload en el JSON exportado");
        let _ = std::fs::remove_file(&tmp);
    }

    /// T024 (TOCTOU residual): la revalidación del confinamiento está PEGADA a la escritura. Si el
    /// `parent` real del `out_path` cae FUERA de la `base_dir` confinada permitida, `export_audit`
    /// DEBE abortar SIN escribir el archivo. Acá lo simulamos pasando un `base_dir` DISTINTO del dir
    /// donde realmente vive el archivo (parent canónico ∉ base canónica), que es exactamente el caso
    /// que un symlink plantado produciría a nivel de canonicalización.
    #[test]
    fn export_aborts_when_parent_outside_base_without_writing() {
        let db = test_db();
        seed_at(&db, "compare", "g1", "h1", "2026-05-30 10:00:00");

        // `outside` es donde apuntaría el destino tras resolver un symlink: fuera de la base.
        let outside = std::env::temp_dir().join(format!(
            "furx-export-outside-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        // `base` es la base confinada permitida (separada de `outside`).
        let base = std::env::temp_dir().join(format!(
            "furx-export-base-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&base).unwrap();

        let out_path = outside.join("escape.ndjson");
        let _ = std::fs::remove_file(&out_path);
        // El parent real (outside) NO está dentro de la base permitida → debe abortar.
        let err = export_audit(&db, ExportScope::All, ExportFormat::Ndjson, &out_path, &base)
            .unwrap_err();
        assert!(
            err.to_string().contains("fuera del directorio permitido"),
            "export debe abortar por confinamiento, dio: {err}"
        );
        // Y CRÍTICO: el archivo NO se escribió (la revalidación corre ANTES del open).
        assert!(
            !out_path.exists(),
            "el archivo de export NO debe existir cuando el parent cae fuera de la base"
        );

        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Variante con SYMLINK real (skip-if-unsupported, como el patrón de `confined_subpath_tests`):
    /// el dir destino es un symlink que resuelve FUERA de la base permitida → `export_audit` aborta
    /// sin escribir. Esto reproduce el TOCTOU real (symlink plantado entre el confirm del caller y el
    /// open), ahora cerrado por la revalidación canonicalize-parent-justo-antes-del-open.
    #[test]
    #[cfg(unix)] // usa symlinks POSIX; en Windows requieren privilegio elevado → se omite
    fn export_aborts_on_symlinked_parent_outside_base() {
        let db = test_db();
        seed_at(&db, "compare", "g1", "h1", "2026-05-30 10:00:00");

        let base = std::env::temp_dir().join(format!(
            "furx-export-symbase-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let outside = std::env::temp_dir().join(format!(
            "furx-export-symout-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        // `base/link` es un symlink que apunta a `outside` (afuera de la base).
        let link = base.join("link");
        if std::os::unix::fs::symlink(&outside, &link).is_err() {
            eprintln!("SKIP export_aborts_on_symlinked_parent_outside_base: sin symlinks");
            let _ = std::fs::remove_dir_all(&base);
            let _ = std::fs::remove_dir_all(&outside);
            return;
        }
        // El parent del out_path es `base/link` → canonicaliza a `outside` → fuera de la base.
        let out_path = link.join("escape.ndjson");
        let _ = std::fs::remove_file(&out_path);
        let err = export_audit(&db, ExportScope::All, ExportFormat::Ndjson, &out_path, &base)
            .unwrap_err();
        assert!(
            err.to_string().contains("fuera del directorio permitido"),
            "symlink-escape debe abortar el export, dio: {err}"
        );
        // El archivo no debe existir (ni en outside ni vía el link).
        assert!(!out_path.exists(), "no debe haberse escrito el archivo tras el symlink-escape");
        assert!(
            !outside.join("escape.ndjson").exists(),
            "no debe haberse escrito a través del symlink afuera de la base"
        );

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
