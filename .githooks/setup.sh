#!/bin/bash
# Setup script to install git hooks and development tools

# Ensure cargo/rustup are on PATH (rustup installs to ~/.cargo/bin)
export PATH="$HOME/.cargo/bin:$PATH"

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${BLUE}Setting up Fractalaw development environment...${NC}"
echo ""

# Ensure we're in the repo root
GIT_DIR=$(git rev-parse --git-dir 2>/dev/null)
if [ $? -ne 0 ]; then
    echo "Error: Not in a git repository"
    exit 1
fi

# 1. Configure git to use our hooks directory
echo -e "${BLUE}[1/3] Installing git hooks...${NC}"
git config core.hooksPath .githooks
echo -e "${GREEN}  ✓ Git hooks path set to .githooks${NC}"

# 2. Check for required cargo tools
echo -e "${BLUE}[2/3] Checking cargo tools...${NC}"

MISSING_TOOLS=()

if ! command -v cargo-audit > /dev/null 2>&1; then
    MISSING_TOOLS+=("cargo-audit")
fi

if ! command -v cargo-machete > /dev/null 2>&1; then
    MISSING_TOOLS+=("cargo-machete")
fi

if [ ${#MISSING_TOOLS[@]} -gt 0 ]; then
    echo -e "${YELLOW}  Missing tools: ${MISSING_TOOLS[*]}${NC}"
    echo -e "${BLUE}  Installing...${NC}"
    cargo install "${MISSING_TOOLS[@]}"
    echo -e "${GREEN}  ✓ Tools installed${NC}"
else
    echo -e "${GREEN}  ✓ All cargo tools present (cargo-audit, cargo-machete)${NC}"
fi

# 3. Verify rustfmt and clippy components
echo -e "${BLUE}[3/3] Checking rustup components...${NC}"

MISSING_COMPONENTS=()

if ! rustup component list --installed | grep -q "rustfmt"; then
    MISSING_COMPONENTS+=("rustfmt")
fi

if ! rustup component list --installed | grep -q "clippy"; then
    MISSING_COMPONENTS+=("clippy")
fi

if [ ${#MISSING_COMPONENTS[@]} -gt 0 ]; then
    echo -e "${YELLOW}  Missing components: ${MISSING_COMPONENTS[*]}${NC}"
    rustup component add "${MISSING_COMPONENTS[@]}"
    echo -e "${GREEN}  ✓ Components installed${NC}"
else
    echo -e "${GREEN}  ✓ All rustup components present (rustfmt, clippy)${NC}"
fi

echo ""
echo -e "${GREEN}✓ Development environment ready!${NC}"
echo ""
echo -e "${BLUE}Git hooks active:${NC}"
echo "  pre-commit : fmt, check, clippy (fast — runs on every commit)"
echo "  pre-push   : test, audit, machete, doc (thorough — runs before push)"
echo ""
echo "To bypass hooks (not recommended):"
echo "  git commit --no-verify"
echo "  git push --no-verify"
