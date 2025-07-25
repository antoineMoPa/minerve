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
- [OpenAI API Key](https://platform.openai.com/signup)

## Configuration

Create a `.env` file in your home directory with the following contents:

```
OPENAI_API_KEY=your_openai_api_key
OPENAI_BASE_URL=https://api.openai.com/v1/
```

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
