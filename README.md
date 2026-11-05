# Minerve
Minerve is an open source rust-based code assistant.

## Features

- **Terminal UI**: Provides an interactive text-based user interface.
- **Headless Mode**: Run prompts in a non-interactive mode.
- **Persistent Chat History**: Allows scrolling through previous conversations.

<p align="center">
    <img src="./assets/minerve.gif" alt="demo" width="700"/>
</p>

## Installation

Install the latest release on macOS (Apple Silicon) or Linux (x64/ARM64) with:

```bash
curl -fsSL https://github.com/antoineMoPa/minerve/releases/latest/download/install.sh | sh
```

## Build from source & hack

Clone the repository and build the project using `make`:

```bash
git clone git@github.com:antoineMoPa/minerve.git
cd minerve
make install
```

This will handle the building and installation of the project.

## Prerequisites

- Rust
- An [OpenRouter API key](https://openrouter.ai/keys)

## Usage

Run the terminal UI with:

```
minerve
```

For executing a command with a specific prompt:

```
minerve "Your prompt here"
```

Add extra system prompts after the default one with `-s` (repeatable).


```
minerve -s "Answer in French"
```

## MCP servers

Minerve can use the tools of [MCP](https://modelcontextprotocol.io) servers. List them in
`~/.minerve/mcp.json` or in a project's `./.mcp.json` (same shape as Claude Code's; the project file wins for a server named in both):

```json
{
  "mcpServers": {
    "filesystem": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "."] },
    "remote": { "type": "http", "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer ..." } }
  }
}
```

Servers without `type` run as a child process (stdio). `"type": "http"` uses Streamable HTTP.
Their tools reach the model as `mcp__<server>__<tool>`.

Type `/mcp` to manage servers: see each one's state and tools, connect or disconnect it for the session, remove it from its config file, or add a new one (saved to `~/.minerve/mcp.json`). In the add form, a URL makes an HTTP server and anything else is a command line. The optional third field takes `KEY=value` pairs separated by commas, used as env vars for a command or headers for a URL.

## Credits

Minerva boot screen is based on an image by wikipedia user JackyM59. [[source]](https://ast.wikipedia.org/wiki/Minerva#/media/Ficheru:Statue_de_la_d%C3%A9esse_Minerve_-_Mus%C3%A9e_du_Louvre_(Ma_332).jpg)

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.

