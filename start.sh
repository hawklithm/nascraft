#!/bin/bash
set -e

# Nascraft 启动管理脚本
# 功能: 交互式配置 + 后台启动 + cron 保活

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PID_FILE="$SCRIPT_DIR/nascraft.pid"
LOG_FILE="$SCRIPT_DIR/nascraft.log"
ENV_FILE="$SCRIPT_DIR/.env"

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info() {
    echo -e "${GREEN}[INFO] $*${NC}"
}

warn() {
    echo -e "${YELLOW}[WARN] $*${NC}"
}

error() {
    echo -e "${RED}[ERROR] $*${NC}"
}

# 读取现有配置
read_existing_config() {
    if [ -f "$ENV_FILE" ]; then
        while IFS='=' read -r key value; do
            case "$key" in
                DATABASE_URL) DATABASE_URL="$value" ;;
                NASCRAFT_PORT) NASCRAFT_PORT="$value" ;;
                NASCRAFT_ENABLE_DLNA_REMOTE) NASCRAFT_ENABLE_DLNA_REMOTE="$value" ;;
            esac
        done < <(grep -v '^#' "$ENV_FILE")
        return 0
    else
        return 1
    fi
}

# 询问用户
ask() {
    local prompt="$1"
    local default="$2"
    if [ -n "$default" ]; then
        read -p "$prompt [$default]: " input
        if [ -z "$input" ]; then
            echo "$default"
        else
            echo "$input"
        fi
    else
        read -p "$prompt: " input
        echo "$input"
    fi
}

# 询问 yes/no
ask_yesno() {
    local prompt="$1"
    local default="$2"
    if [ "$default" = "y" ]; then
        read -p "$prompt [Y/n]: " input
        if [ -z "$input" ] || [ "${input,,}" = "y" ] || [ "${input,,}" = "yes" ]; then
            return 0
        else
            return 1
        fi
    else
        read -p "$prompt [y/N]: " input
        if [ "${input,,}" = "y" ] || [ "${input,,}" = "yes" ]; then
            return 0
        else
            return 1
        fi
    fi
}

# 检查服务是否运行
is_running() {
    if [ -f "$PID_FILE" ]; then
        local pid=$(cat "$PID_FILE")
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
        local pid=$(cat "$PID_FILE")
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

    info "正在后台启动 Nascraft..."
    cd "$SCRIPT_DIR"
    nohup ./nascraft >> "$LOG_FILE" 2>&1 &
    local pid=$!
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

    local cron_entry="*/5 * * * * $SCRIPT_DIR/start.sh keepalive >> $SCRIPT_DIR/cron.log 2>&1"

    # 获取当前用户的 crontab
    local temp_cron=$(mktemp)
    crontab -l 2>/dev/null | grep -v "nascraft.*keepalive" > "$temp_cron"

    # 添加新的 cron 任务
    echo "$cron_entry" >> "$temp_cron"

    # 安装新的 crontab
    crontab "$temp_cron"
    rm -f "$temp_cron"

    info "cron 保活任务已设置 (每5分钟检查一次)"
}

# 移除 cron 保活
remove_cron() {
    info "正在移除 cron 保活任务..."

    local temp_cron=$(mktemp)
    crontab -l 2>/dev/null | grep -v "nascraft.*keepalive" > "$temp_cron"
    crontab "$temp_cron"
    rm -f "$temp_cron"

    info "cron 保活任务已移除"
}

# 保活检查
keepalive_check() {
    if ! is_running; then
        warn "Nascraft 服务未运行，正在重启..."
        stop_service
        start_service
    else
        info "Nascraft 服务运行正常 (PID: $(cat $PID_FILE 2>/dev/null))"
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
    DATABASE_URL=$(ask "请输入数据库文件路径" "${DATABASE_URL:-$DATABASE_URL_DEFAULT}")

    # 服务端口
    NASCRAFT_PORT=$(ask "请输入服务监听端口" "${NASCRAFT_PORT:-$NASCRAFT_PORT_DEFAULT}")

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
    cat > "$ENV_FILE" << EOF
# Nascraft configuration
# Generated by start.sh on $(date)

DATABASE_URL=sqlite:$DATABASE_URL
NASCRAFT_PORT=$NASCRAFT_PORT
NASCRAFT_ENABLE_DLNA_REMOTE=$NASCRAFT_ENABLE_DLNA_REMOTE

# Optional: mDNS 服务类型
# NASCRAFT_MDNS_SERVICE_TYPE=_nascraft._tcp.local.

# Optional: mDNS 实例名称
# NASCRAFT_MDNS_INSTANCE=nascraft

# Optional: UDP 发现端口
# NASCRAFT_UDP_DISCOVERY_PORT=53530
EOF

    # 创建必要的目录
    mkdir -p "$SCRIPT_DIR/data"
    mkdir -p "$SCRIPT_DIR/uploads"
    mkdir -p "$SCRIPT_DIR/thumbnails"

    info "配置已保存到 $ENV_FILE"
    echo ""
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
        info "服务状态: 运行中 (PID: $(cat $PID_FILE))"
    else
        warn "服务状态: 已停止"
    fi
    echo ""
    # 检查 cron
    if crontab -l 2>/dev/null | grep -q "nascraft.*keepalive"; then
        info "Cron 保活: 已启用"
    else
        warn "Cron 保活: 未启用"
    fi
    echo ""
}

# ============ 主逻辑 ============

case "${1:-configure}" in
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
