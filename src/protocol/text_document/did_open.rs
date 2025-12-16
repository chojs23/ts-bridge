//! Handles `textDocument/didOpen` by updating tsserver’s open files list and
//! pushing a configure request, mirroring the Lua implementation.

pub fn handle() {
    todo!("Translate didOpen params into tsserver UpdateOpen + Configure commands");
}
