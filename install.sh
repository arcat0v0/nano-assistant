#!/bin/bash
set -e

INSTALL_DIR="$HOME/.local/bin"
CONFIG_DIR="$HOME/.config/nano-assistant"
CONFIG_FILE="$CONFIG_DIR/config.toml"

echo "Installing nano-assistant..."

# Check if cargo is installed
if ! command -v cargo &> /dev/null; then
    echo "Error: cargo is not installed. Please install Rust first: https://rustup.rs/"
    exit 1
fi

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ]; then
    echo "Error: Cargo.toml not found. Please run this script from the project directory."
    exit 1
fi

# Build the project
echo "Building project..."
if ! cargo build --release; then
    echo "Error: Build failed."
    exit 1
fi

# Check if binary exists
BINARY="target/release/na"
if [ ! -f "$BINARY" ]; then
    echo "Error: Binary not found at $BINARY"
    exit 1
fi

# Create install directory if it doesn't exist
mkdir -p "$INSTALL_DIR"

# Copy binary and create symlink
echo "Installing to $INSTALL_DIR..."
cp "$BINARY" "$INSTALL_DIR/na"
ln -sf "$INSTALL_DIR/na" "$INSTALL_DIR/nano-assistant"
chmod +x "$INSTALL_DIR/na"

# Create config directory
mkdir -p "$CONFIG_DIR"

# Create default config if it doesn't exist
if [ ! -f "$CONFIG_FILE" ]; then
    echo "Creating default config at $CONFIG_FILE..."
    cat > "$CONFIG_FILE" << 'EOF'
[provider]
provider = "openai"
model = "gpt-4o-mini"
api_key = ""  # Set your API key here or via NA_API_KEY env var

[memory]
enabled = true

[security]
mode = "confirm"  # direct | confirm | whitelist
whitelist = ["ls", "cat", "grep", "echo", "pwd", "cd"]

[behavior]
streaming = true
max_iterations = 10
EOF
fi

# Add ~/.local/bin to PATH in shell rc files
add_to_path() {
    local rc_file="$1"
    local path_entry='export PATH="$HOME/.local/bin:$PATH"'
    
    if [ -f "$rc_file" ]; then
        if ! grep -q '~/.local/bin' "$rc_file" && ! grep -q '$HOME/.local/bin' "$rc_file"; then
            echo "" >> "$rc_file"
            echo "# Added by nano-assistant installer" >> "$rc_file"
            echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$rc_file"
            echo "Added PATH to $rc_file"
        fi
    else
        echo "" >> "$rc_file"
        echo "# Added by nano-assistant installer" >> "$rc_file"
        echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$rc_file"
        echo "Created $rc_file with PATH configuration"
    fi
}

# Update shell rc files
for rc_file in "$HOME/.bashrc" "$HOME/.zshrc"; do
    if [ -f "$rc_file" ]; then
        add_to_path "$rc_file"
    fi
done

echo ""
echo "Installation complete!"
echo ""
echo "Usage:"
echo "  na \"list all docker containers\"  # Single command mode"
echo "  na                           # Interactive REPL mode"
echo "  na --config                  # Open config file in editor"
echo ""
echo "Make sure ~/.local/bin is in your PATH. Run:"
echo "  source ~/.bashrc  # or ~/.zshrc"
echo "Or add this line to your shell config:"
echo '  export PATH="$HOME/.local/bin:$PATH"'
