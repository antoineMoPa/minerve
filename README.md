# Minerve
Minerve is an open source rust-based code assistant.

## Features

- **Terminal UI**: Provides an interactive text-based user interface.
- **Headless Mode**: Run prompts in a non-interactive mode.
- **Persistent Chat History**: Allows scrolling through previous conversations.

## Installation

To set up Minerve, clone the repository and build the project using `make`:

```bash
git clone <repo-url>
cd <repo-name>
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

`MINERVE_MODEL` overrides the default model. Any model id from https://openrouter.ai/models that
supports tool calling works, for example `MINERVE_MODEL=~anthropic/claude-sonnet-latest`.

The status bar shows tokens sent and received and the session cost in USD, as reported by
OpenRouter for each request. Headless runs print the same line when they finish.

## Usage

Run the terminal UI with:

```
minerve
```

For executing a command with a specific prompt:

```
minerve -p "Your query here"
```

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.
