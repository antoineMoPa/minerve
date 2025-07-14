use crate::tools::Tool;
use std::collections::HashMap;
use std::sync::Arc;

use super::get_general_context_tool::GetGeneralContext;
use super::git_diff_cached_tool::GitDiffCachedTool;
use super::git_diff_tool::GitDiffTool;
use super::git_status_tool::GitStatusTool;
use super::replace_content_tool::ReplaceContentTool;
use super::run_cargo_check_tool::RunCargoCheckTool;
use super::run_shell_command_tool::RunShellCommandTool;
use super::search_for_path_pattern_tool::SearchForPathPatternTool;
use super::search_for_string_tool::SearchForStringTool;
use super::set_whole_file_contents_tool::SetWholeFileContentsTool;
use super::list_files_tool::ListFilesTool;
use super::show_file_tool::ShowFileTool;
use super::extract_structure_tool::ExtractStructureTool;

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
    map.insert("show_file", Arc::new(ShowFileTool));
    map.insert("replace_content", Arc::new(ReplaceContentTool));
    map.insert("run_cargo_check", Arc::new(RunCargoCheckTool));
    map.insert("run_shell_command", Arc::new(RunShellCommandTool));
    map.insert("set_whole_file_contents", Arc::new(SetWholeFileContentsTool));
    map.insert("extract_structure", Arc::new(ExtractStructureTool));

    map
}
