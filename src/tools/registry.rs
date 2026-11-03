use crate::tools::Tool;
use std::collections::HashMap;
use std::sync::Arc;

use super::edit_tool::EditTool;
use super::extract_structure_tool::ExtractStructureTool;
use super::get_general_context_tool::GetGeneralContext;
use super::get_url_tool::GetUrlTool;
use super::git_diff_cached_tool::GitDiffCachedTool;
use super::git_diff_tool::GitDiffTool;
use super::git_status_tool::GitStatusTool;
use super::list_files_tool::ListFilesTool;
use super::read_tool::ReadTool;
use super::run_cargo_check_tool::RunCargoCheckTool;
use super::run_shell_command_tool::RunShellCommandTool;
use super::search_for_path_pattern_tool::SearchForPathPatternTool;
use super::search_for_string_tool::SearchForStringTool;
use super::write_tool::WriteTool;
use super::compile_typescript_project_tool::CompileTypescriptProjectTool;

pub fn get_tool_registry() -> HashMap<&'static str, Arc<dyn Tool>> {
    let mut map: HashMap<&'static str, Arc<dyn Tool>> = HashMap::new();

    map.insert("get_general_context", Arc::new(GetGeneralContext));
    map.insert("search_for_string", Arc::new(SearchForStringTool));
    map.insert(
        "search_for_path_pattern",
        Arc::new(SearchForPathPatternTool),
    );
    map.insert("list_files", Arc::new(ListFilesTool));
    map.insert("git_status", Arc::new(GitStatusTool));
    map.insert("git_diff", Arc::new(GitDiffTool));
    map.insert("git_diff_cached", Arc::new(GitDiffCachedTool));
    map.insert("read", Arc::new(ReadTool));
    map.insert("edit", Arc::new(EditTool));
    map.insert("write", Arc::new(WriteTool));
    map.insert("run_cargo_check", Arc::new(RunCargoCheckTool));
    map.insert("run_shell_command", Arc::new(RunShellCommandTool));
    map.insert("extract_structure", Arc::new(ExtractStructureTool));
    map.insert("compile_typescript_project", Arc::new(CompileTypescriptProjectTool));
    map.insert("get_url", Arc::new(GetUrlTool));
    map
}
