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

The installer downloads the release archive, verifies its SHA-256 checksum, and installs `minerve` to `~/.local/bin`. Set `MINERVE_INSTALL_DIR` to change the location or `MINERVE_VERSION=vX.Y.Z` to pin a release.

To build from source instead, clone the repository and build the project using `make`:

```bash
git clone git@github.com:antoineMoPa/minerve.git
cd minerve
make install
```

This will handle the building and installation of the project.

## Prerequisites

- Rust
- An [OpenRouter API key](https://openrouter.ai/keys)

## Configuration

Create a `.env` file in your home directory:

```
OPENROUTER_API_KEY=your_openrouter_api_key
```

Switch models at runtime with `/model`: type to filter the OpenRouter list, the 4 most recently used models stay on top (saved in `~/.minerve/recent_models.json`).

`MINERVE_MODEL` overrides the default model. Any model id from https://openrouter.ai/models that
supports tool calling works, for example `MINERVE_MODEL=~anthropic/claude-sonnet-latest`.

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

## Credits

Minerva boot screen is based on an image by wikipedia user JackyM59. [[source]](https://ast.wikipedia.org/wiki/Minerva#/media/Ficheru:Statue_de_la_d%C3%A9esse_Minerve_-_Mus%C3%A9e_du_Louvre_(Ma_332).jpg)

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.

