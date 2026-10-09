#!/bin/sh
set -e

# Nascraft 启动管理脚本
# 功能: 交互式配置 + 后台启动 + cron 保活

# Get script directory (POSIX compatible)
SCRIPT_DIR=$(dirname "$0")
SCRIPT_DIR=$(cd "$SCRIPT_DIR" && pwd)
PID_FILE="$SCRIPT_DIR/nascraft.pid"
LOG_FILE="$SCRIPT_DIR/nascraft.log"
ENV_FILE="$SCRIPT_DIR/.env"

# 颜色定义 (only if stdout is terminal)
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    NC='\033[0m' # No Color
else
    RED=''
    GREEN=''
    YELLOW=''
    NC=''
fi

info() {
    printf "${GREEN}[INFO] %s${NC}\n" "$*"
}

warn() {
    printf "${YELLOW}[WARN] %s${NC}\n" "$*"
}

error() {
    printf "${RED}[ERROR] %s${NC}\n" "$*"
}

# 读取现有配置
read_existing_config() {
    if [ -f "$ENV_FILE" ]; then
        # POSIX: use grep and while loop without process substitution
        grep -v '^#' "$ENV_FILE" | while IFS='=' read -r key value; do
            case "$key" in
                DATABASE_URL) DATABASE_URL="$value" ;;
                NASCRAFT_PORT) NASCRAFT_PORT="$value" ;;
                NASCRAFT_ENABLE_DLNA_REMOTE) NASCRAFT_ENABLE_DLNA_REMOTE="$value" ;;
            esac
        done
        return 0
    else
        return 1
    fi
}

# 询问用户
# 输出提示到 stdout，读取用户输入，结果存在全局变量 ASK_RESULT
ask() {
    prompt="$1"
    default="$2"
    if [ -n "$default" ]; then
        printf "%s [%s]: " "$prompt" "$default"
        read -r input
        if [ -z "$input" ]; then
            ASK_RESULT="$default"
        else
            ASK_RESULT="$input"
        fi
    else
        printf "%s: " "$prompt"
        read -r input
        ASK_RESULT="$input"
    fi
}

# 询问 yes/no - POSIX compatible lowercase conversion
to_lower() {
    echo "$1" | tr '[:upper:]' '[:lower:]'
}

ask_yesno() {
    prompt="$1"
    default="$2"
    if [ "$default" = "y" ]; then
        printf "%s [Y/n]: " "$prompt"
        read -r input
        if [ -z "$input" ]; then
            return 0
        fi
        input=$(to_lower "$input")
        if [ "$input" = "y" ] || [ "$input" = "yes" ]; then
            return 0
        else
            return 1
        fi
    else
        printf "%s [y/N]: " "$prompt"
        read -r input
        if [ -z "$input" ]; then
            return 1
        fi
        input=$(to_lower "$input")
        if [ "$input" = "y" ] || [ "$input" = "yes" ]; then
            return 0
        else
            return 1
        fi
    fi
}

# 检查服务是否运行
is_running() {
    if [ -f "$PID_FILE" ]; then
        pid=$(cat "$PID_FILE")
        if kill -0 "$pid" 2>/dev/null; then
            return 0
        else
            # PID 文件存在但进程不存在
            rm -f "$PID_FILE"
            return 1
        fi
    fi
    return 1
}

# 停止服务
stop_service() {
    if [ -f "$PID_FILE" ]; then
        pid=$(cat "$PID_FILE")
        if kill -0 "$pid" 2>/dev/null; then
            info "正在停止 Nascraft 服务 (PID: $pid)..."
            kill "$pid"
            sleep 2
            if kill -0 "$pid" 2>/dev/null; then
                kill -9 "$pid"
                sleep 1
            fi
        fi
        rm -f "$PID_FILE"
        info "服务已停止"
    else
        warn "服务没有在运行"
    fi
}

# 启动服务
start_service() {
    if is_running; then
        warn "Nascraft 已经在运行了"
        return 1
    fi

    # 确保 ffmpeg 可用（视频缩略图依赖），并写 FFMPEG_PATH 到 .env
    ensure_ffmpeg

    info "正在后台启动 Nascraft..."
    cd "$SCRIPT_DIR"
    nohup ./nascraft >> "$LOG_FILE" 2>&1 &
    pid=$!
    echo "$pid" > "$PID_FILE"
    info "服务已启动，PID: $pid"
    info "日志文件: $LOG_FILE"
    sleep 2
    if kill -0 "$pid" 2>/dev/null; then
        info "启动成功 ✓"
    else
        error "启动失败，请检查日志: $LOG_FILE"
        return 1
    fi
}

# 设置 cron 保活
setup_cron() {
    info "正在设置 cron 保活任务..."

    cron_entry="*/5 * * * * $SCRIPT_DIR/start.sh keepalive >> $SCRIPT_DIR/cron.log 2>&1"

    # 获取当前用户的 crontab
    temp_cron=$(mktemp)
    # When crontab is empty or grep filtered everything, still write to temp file
    {
        crontab -l 2>/dev/null || true
    } | {
        grep -v "start.sh.*keepalive" || true
    } > "$temp_cron"

    # 添加新的 cron 任务
    echo "$cron_entry" >> "$temp_cron"

    # 安装新的 crontab - keep temp file until crontab installs it
    if crontab "$temp_cron"; then
        rm -f "$temp_cron"
        info "cron 保活任务已设置 (每5分钟检查一次)"
    else
        rm -f "$temp_cron"
        error "设置 cron 保活失败，请手动添加："
        echo "  $cron_entry"
    fi
}

# 移除 cron 保活
remove_cron() {
    info "正在移除 cron 保活任务..."

    temp_cron=$(mktemp)
    {
        crontab -l 2>/dev/null || true
    } | {
        grep -v "start.sh.*keepalive" || true
    } > "$temp_cron"
    crontab "$temp_cron"
    rm -f "$temp_cron"

    info "cron 保活任务已移除"
}

# 检测操作系统类型
detect_os() {
    case "$(uname -s)" in
        Darwin) echo "macos" ;;
        Linux) echo "linux" ;;
        *) echo "unknown" ;;
    esac
}

# 检测 Linux 包管理器
detect_pkg_manager() {
    if command -v apt-get >/dev/null 2>&1; then
        echo "apt"
    elif command -v dnf >/dev/null 2>&1; then
        echo "dnf"
    elif command -v yum >/dev/null 2>&1; then
        echo "yum"
    elif command -v apk >/dev/null 2>&1; then
        echo "apk"
    elif command -v pacman >/dev/null 2>&1; then
        echo "pacman"
    elif command -v zypper >/dev/null 2>&1; then
        echo "zypper"
    else
        echo "unknown"
    fi
}

# 写入/更新 .env 中的某个 key=value（POSIX sh，兼容路径含 /）
set_env_key() {
    key="$1"
    value="$2"
    if [ ! -f "$ENV_FILE" ]; then
        touch "$ENV_FILE"
    fi
    if grep -q "^${key}=" "$ENV_FILE" 2>/dev/null; then
        temp_env=$(mktemp)
        sed "s|^${key}=.*|${key}=${value}|" "$ENV_FILE" > "$temp_env"
        mv "$temp_env" "$ENV_FILE"
    else
        echo "${key}=${value}" >> "$ENV_FILE"
    fi
}

# 按平台自动安装 ffmpeg（失败不阻断，仅告警）
install_ffmpeg() {
    os=$(detect_os)
    case "$os" in
        macos)
            if command -v brew >/dev/null 2>&1; then
                info "使用 Homebrew 安装 ffmpeg（可能需要几分钟）..."
                brew install ffmpeg || warn "brew install ffmpeg 失败，请手动安装"
            else
                warn "未检测到 Homebrew，请先安装 Homebrew 后手动执行: brew install ffmpeg"
            fi
            ;;
        linux)
            pkg=$(detect_pkg_manager)
            case "$pkg" in
                apt)
                    if [ "$(id -u)" -eq 0 ]; then
                        apt-get update >/dev/null 2>&1 && apt-get install -y ffmpeg >/dev/null 2>&1 || warn "apt 安装 ffmpeg 失败"
                    else
                        sudo -n apt-get install -y ffmpeg >/dev/null 2>&1 || warn "需要 sudo 权限安装 ffmpeg，请手动执行: sudo apt-get install -y ffmpeg"
                    fi
                    ;;
                dnf)
                    if [ "$(id -u)" -eq 0 ]; then
                        dnf install -y ffmpeg >/dev/null 2>&1 || warn "dnf 安装 ffmpeg 失败"
                    else
                        sudo -n dnf install -y ffmpeg >/dev/null 2>&1 || warn "请手动执行: sudo dnf install -y ffmpeg"
                    fi
                    ;;
                yum)
                    if [ "$(id -u)" -eq 0 ]; then
                        yum install -y ffmpeg >/dev/null 2>&1 || warn "yum 安装 ffmpeg 失败"
                    else
                        sudo -n yum install -y ffmpeg >/dev/null 2>&1 || warn "请手动执行: sudo yum install -y ffmpeg"
                    fi
                    ;;
                apk)
                    apk add --no-cache ffmpeg >/dev/null 2>&1 || warn "apk 安装 ffmpeg 失败"
                    ;;
                pacman)
                    if [ "$(id -u)" -eq 0 ]; then
                        pacman -S --noconfirm ffmpeg >/dev/null 2>&1 || warn "pacman 安装 ffmpeg 失败"
                    else
                        sudo -n pacman -S --noconfirm ffmpeg >/dev/null 2>&1 || warn "请手动执行: sudo pacman -S ffmpeg"
                    fi
                    ;;
                *)
                    warn "未识别的 Linux 包管理器，请手动安装 ffmpeg"
                    ;;
            esac
            ;;
        *)
            warn "未知操作系统，请手动安装 ffmpeg 并在 .env 中设置 FFMPEG_PATH"
            ;;
    esac
}

# 确保 ffmpeg 可用：检测 → 自动安装 → 写 FFMPEG_PATH 到 .env
# 写绝对路径是为了让 cron 保活（PATH 精简）下服务端也能精确定位 ffmpeg
ensure_ffmpeg() {
    ffmpeg_bin=""

    # 1. 优先使用 .env 里已配置的 FFMPEG_PATH
    if [ -f "$ENV_FILE" ]; then
        configured_ffmpeg=$(grep '^FFMPEG_PATH=' "$ENV_FILE" 2>/dev/null | head -1 | cut -d= -f2-)
        if [ -n "$configured_ffmpeg" ] && [ -x "$configured_ffmpeg" ]; then
            ffmpeg_bin="$configured_ffmpeg"
        fi
    fi

    # 2. 尝试 PATH
    if [ -z "$ffmpeg_bin" ]; then
        ffmpeg_bin=$(command -v ffmpeg 2>/dev/null || true)
    fi

    # 3. 都没有 → 自动安装
    if [ -z "$ffmpeg_bin" ]; then
        warn "未检测到 ffmpeg（视频缩略图需要它），尝试自动安装..."
        install_ffmpeg
        ffmpeg_bin=$(command -v ffmpeg 2>/dev/null || true)
    fi

    # 4. 写入/更新 FFMPEG_PATH（仅在变化时写，避免每次保活重写 .env）
    if [ -n "$ffmpeg_bin" ]; then
        current_env=$(grep '^FFMPEG_PATH=' "$ENV_FILE" 2>/dev/null | head -1 | cut -d= -f2-)
        if [ "$current_env" != "$ffmpeg_bin" ]; then
            set_env_key "FFMPEG_PATH" "$ffmpeg_bin"
            info "已设置 FFMPEG_PATH=$ffmpeg_bin"
        else
            info "ffmpeg 就绪: $ffmpeg_bin"
        fi
    else
        warn "ffmpeg 不可用，视频缩略图功能将不可用（不影响其他功能）"
    fi
}

# 保活检查
keepalive_check() {
    if ! is_running; then
        warn "Nascraft 服务未运行，正在重启..."
        stop_service
        start_service
    else
        pid=$(cat "$PID_FILE" 2>/dev/null)
        info "Nascraft 服务运行正常 (PID: $pid)"
    fi
}

# 交互式配置
configure() {
    echo ""
    echo "=== Nascraft 配置向导 ==="
    echo ""

    # 默认值
    DATABASE_URL_DEFAULT="./nascraft.db"
    NASCRAFT_PORT_DEFAULT="8080"
    NASCRAFT_ENABLE_DLNA_REMOTE_DEFAULT="false"

    # 读取现有配置
    if read_existing_config; then
        info "找到现有配置文件，将显示当前值，回车保持不变"
        echo ""
    else
        info "未找到现有配置，开始设置..."
        echo ""
    fi

    # 数据库路径
    ask "请输入数据库文件路径" "${DATABASE_URL:-$DATABASE_URL_DEFAULT}"
    DATABASE_URL="$ASK_RESULT"

    # 服务端口
    ask "请输入服务监听端口" "${NASCRAFT_PORT:-$NASCRAFT_PORT_DEFAULT}"
    NASCRAFT_PORT="$ASK_RESULT"

    # DLNA 远程
    if [ -z "$NASCRAFT_ENABLE_DLNA_REMOTE" ]; then
        NASCRAFT_ENABLE_DLNA_REMOTE="$NASCRAFT_ENABLE_DLNA_REMOTE_DEFAULT"
    fi
    current_yn="N"
    if [ "$NASCRAFT_ENABLE_DLNA_REMOTE" = "true" ]; then
        current_yn="Y"
    fi
    if ask_yesno "是否启用 DLNA 远程投屏功能" "$current_yn"; then
        NASCRAFT_ENABLE_DLNA_REMOTE="true"
    else
        NASCRAFT_ENABLE_DLNA_REMOTE="false"
    fi

    echo ""
    info "配置汇总:"
    echo "  DATABASE_URL: $DATABASE_URL"
    echo "  NASCRAFT_PORT: $NASCRAFT_PORT"
    echo "  NASCRAFT_ENABLE_DLNA_REMOTE: $NASCRAFT_ENABLE_DLNA_REMOTE"
    echo ""

    if ! ask_yesno "确认保存配置" "y"; then
        info "配置已取消"
        return 1
    fi

    # 写入 .env 文件
    {
        echo "# Nascraft configuration"
        echo "# Generated by start.sh on $(date)"
        echo ""
        echo "DATABASE_URL=sqlite:$DATABASE_URL"
        echo "NASCRAFT_PORT=$NASCRAFT_PORT"
        echo "NASCRAFT_ENABLE_DLNA_REMOTE=$NASCRAFT_ENABLE_DLNA_REMOTE"
        echo ""
        echo "# Optional: mDNS 服务类型"
        echo "# NASCRAFT_MDNS_SERVICE_TYPE=_nascraft._tcp.local."
        echo ""
        echo "# Optional: mDNS 实例名称"
        echo "# NASCRAFT_MDNS_INSTANCE=nascraft"
        echo ""
        echo "# Optional: UDP 发现端口"
        echo "# NASCRAFT_UDP_DISCOVERY_PORT=53530"
        echo ""
        echo "# ffmpeg 路径（视频缩略图用，start.sh 会自动检测并填写）"
        echo "# FFMPEG_PATH=/usr/bin/ffmpeg"
    } > "$ENV_FILE"

    # 创建必要的目录
    mkdir -p "$SCRIPT_DIR/data"
    mkdir -p "$SCRIPT_DIR/uploads"
    mkdir -p "$SCRIPT_DIR/thumbnails"

    info "配置已保存到 $ENV_FILE"
    echo ""

    if ask_yesno "是否立即启动服务并设置 cron 保活" "y"; then
        start_service
        setup_cron
    fi

    return 0
}

# 显示帮助
show_help() {
    echo "Nascraft 启动管理脚本"
    echo ""
    echo "用法: ./start.sh [命令]"
    echo ""
    echo "命令:"
    echo "  configure   - 交互式配置（默认，如果没给命令）"
    echo "  start       - 配置后启动服务并设置 cron 保活"
    echo "  stop        - 停止服务并移除 cron 保活"
    echo "  restart     - 重启服务"
    echo "  keepalive   - 保活检查（由 cron 调用）"
    echo "  status      - 检查服务状态"
    echo "  help        - 显示帮助"
    echo ""
}

# 显示状态
show_status() {
    echo "=== Nascraft 状态 ==="
    if is_running; then
        pid=$(cat "$PID_FILE")
        info "服务状态: 运行中 (PID: $pid)"
    else
        warn "服务状态: 已停止"
    fi
    echo ""
    # 检查 cron
    has_cron=0
    if [ -n "$(crontab -l 2>/dev/null | grep "start.sh.*keepalive" || true)" ]; then
        has_cron=1
    fi
    if [ "$has_cron" -eq 1 ]; then
        info "Cron 保活: 已启用"
    else
        warn "Cron 保活: 未启用"
    fi
    echo ""
}

# ============ 主逻辑 ============

# Get command
cmd="${1:-configure}"

case "$cmd" in
    help)
        show_help
        ;;

    status)
        show_status
        ;;

    configure)
        configure
        ;;

    start)
        configure && start_service && setup_cron
        ;;

    stop)
        stop_service
        remove_cron
        ;;

    restart)
        stop_service
        sleep 1
        start_service
        setup_cron
        ;;

    keepalive)
        keepalive_check
        ;;

    *)
        error "未知命令: $1"
        echo ""
        show_help
        exit 1
        ;;
esac

exit 0
