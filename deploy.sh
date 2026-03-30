#!/bin/bash
set -e

# Nascraft 部署打包脚本
# 编译项目并打包成可部署的 tar.gz 包

echo "=== Nascraft 部署打包 ==="
echo ""

# 检查是否在正确目录
if [ ! -f "Cargo.toml" ]; then
    echo "错误: 请在 nascraft 目录下运行此脚本"
    exit 1
fi

# 编译 release 版本
echo ">>> 编译 release 版本..."
cargo build --release

if [ $? -ne 0 ]; then
    echo "编译失败！"
    exit 1
fi

echo "编译完成！"
echo ""

# 创建部署目录
DEPLOY_DIR="nascraft-deploy"
mkdir -p $DEPLOY_DIR
mkdir -p $DEPLOY_DIR/data
mkdir -p $DEPLOY_DIR/uploads
mkdir -p $DEPLOY_DIR/thumbnails

# 复制二进制文件
cp target/release/nascraft $DEPLOY_DIR/

# 复制启动脚本
cp start.sh $DEPLOY_DIR/

# 复制示例 .env 文件
if [ -f ".env" ]; then
    cp .env $DEPLOY_DIR/
else
    cp .env.example $DEPLOY_DIR/.env
fi

# 创建打包文件名
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
PACKAGE_NAME="nascraft-deploy-$TIMESTAMP.tar.gz"

# 打包
echo ">>> 创建部署包 $PACKAGE_NAME..."
tar -czf $PACKAGE_NAME -C $DEPLOY_DIR .

echo ""
echo "=== 打包完成 ==="
echo "部署包: $(pwd)/$PACKAGE_NAME"
echo "部署包大小: $(du -h $PACKAGE_NAME | cut -f1)"
echo ""
echo "部署步骤:"
echo "1. 上传 $PACKAGE_NAME 到服务器"
echo "2. 解压: tar -xzf $PACKAGE_NAME"
echo "3. 运行 ./start.sh 进行配置并启动"
echo ""

# 清理临时目录
rm -rf $DEPLOY_DIR

echo "完成！"
