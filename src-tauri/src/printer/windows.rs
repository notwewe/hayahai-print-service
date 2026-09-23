use std::ptr;

use serde_json::json;
use sha2::{Digest, Sha256};
use windows_sys::Win32::Graphics::Printing::{
    ClosePrinter, DOC_INFO_1W, EndDocPrinter, EndPagePrinter, EnumPrintersW, OpenPrinterW,
    PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL, PRINTER_HANDLE, PRINTER_INFO_2W,
    StartDocPrinterW, StartPagePrinter, WritePrinter,
};

use crate::{AgentError, model::PrinterInfo};

use super::SpoolOutcome;

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

unsafe fn from_wide(pointer: *mut u16) -> String {
    if pointer.is_null() {
        return String::new();
    }
    let mut length = 0;
    while unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(pointer, length) })
}

pub async fn discover() -> Result<Vec<PrinterInfo>, AgentError> {
    tauri::async_runtime::spawn_blocking(|| unsafe {
        let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
        let mut needed = 0;
        let mut count = 0;
        EnumPrintersW(
            flags,
            ptr::null(),
            2,
            ptr::null_mut(),
            0,
            &mut needed,
            &mut count,
        );
        if needed == 0 {
            return Ok(Vec::new());
        }
        let mut buffer = vec![0_u8; needed as usize];
        if EnumPrintersW(
            flags,
            ptr::null(),
            2,
            buffer.as_mut_ptr(),
            needed,
            &mut needed,
            &mut count,
        ) == 0
        {
            return Err(AgentError::Printer(
                "Windows printer discovery failed".into(),
            ));
        }
        let rows =
            std::slice::from_raw_parts(buffer.as_ptr() as *const PRINTER_INFO_2W, count as usize);
        Ok(rows
            .iter()
            .map(|row| {
                let name = from_wide(row.pPrinterName);
                let driver = from_wide(row.pDriverName);
                let port = from_wide(row.pPortName);
                let fingerprint = hex::encode(Sha256::digest(
                    format!("{name}\n{driver}\n{port}").as_bytes(),
                ));
                PrinterInfo {
                    queue_id: name.clone(),
                    display_name: name,
                    driver_name: Some(driver),
                    transport: "os-queue".into(),
                    capabilities: json!({ "raw": true, "provider": "winspool", "port": port }),
                    environment_hash: fingerprint,
                    available: true,
                }
            })
            .collect())
    })
    .await
    .map_err(|error| AgentError::Printer(error.to_string()))?
}

pub async fn spool(queue: &str, job_id: &str, bytes: &[u8]) -> SpoolOutcome {
    let queue = queue.to_owned();
    let job_id = job_id.to_owned();
    let bytes = bytes.to_vec();
    tauri::async_runtime::spawn_blocking(move || unsafe {
        let mut printer = PRINTER_HANDLE(ptr::null_mut());
        let queue_wide = wide(&queue);
        if OpenPrinterW(queue_wide.as_ptr(), &mut printer, ptr::null()) == 0 {
            return SpoolOutcome::Failed("Windows could not open the printer queue".into());
        }
        let title = wide(&format!("HayahAI-{job_id}"));
        let raw = wide("RAW");
        let info = DOC_INFO_1W {
            pDocName: title.as_ptr() as *mut _,
            pOutputFile: ptr::null_mut(),
            pDatatype: raw.as_ptr() as *mut _,
        };
        if StartDocPrinterW(printer, 1, &info) == 0 {
            ClosePrinter(printer);
            return SpoolOutcome::Failed("Windows rejected the raw print job".into());
        }
        if StartPagePrinter(printer) == 0 {
            EndDocPrinter(printer);
            ClosePrinter(printer);
            return SpoolOutcome::Unknown("Windows did not start the print page".into());
        }
        let mut written = 0;
        let ok = WritePrinter(
            printer,
            bytes.as_ptr() as *const _,
            bytes.len() as u32,
            &mut written,
        );
        let page_ok = EndPagePrinter(printer);
        let doc_ok = EndDocPrinter(printer);
        ClosePrinter(printer);
        if ok != 0 && written as usize == bytes.len() && page_ok != 0 && doc_ok != 0 {
            SpoolOutcome::Submitted
        } else {
            SpoolOutcome::Unknown(format!(
                "Windows spool result was incomplete ({written}/{} bytes)",
                bytes.len()
            ))
        }
    })
    .await
    .unwrap_or_else(|error| SpoolOutcome::Unknown(error.to_string()))
}
