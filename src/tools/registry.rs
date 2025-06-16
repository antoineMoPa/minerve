use std::collections::HashMap;
use std::sync::Arc;
use crate::tools::Tool;

use super::get_general_context::GetGeneralContext;

pub fn get_tool_registry() -> HashMap<&'static str, Arc<dyn Tool>> {
    let mut map: HashMap<&'static str, Arc<dyn Tool>> = HashMap::new();

    map.insert("get_general_context", Arc::new(GetGeneralContext));
    // map.insert("read_file", Arc::new(ReadFileTool)); etc.

    map
}
