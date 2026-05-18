# ohosHttp v1.3.1 — HarmonyOS HTTP 服务器

## 打包内容

```
ohosHttp-v1.3.1/
├── ohosHttp-aarch64-ohos   # 程序二进制文件 (ARM64)
├── install.sh              # 一键安装脚本
├── uninstall.sh            # 卸载脚本
└── README.txt              # 本说明文件
```

## 系统要求

- HarmonyOS PC / 兼容 Linux 环境
- ARM64 (aarch64) 架构
- 已启用开发者模式和调试桥 (HDC)
- 需要 **root 权限** 执行安装（鸿蒙 PC 权限管控严格）

## 快速安装

1. 将 `ohosHttp-v1.3.1/` 目录通过 HDC 推送到设备：
   ```
   hdc file send ohosHttp-v1.3.1 /data/local/tmp/
   ```

2. 进入目录并使用 **sudo** 执行安装脚本：
   ```
   cd /data/local/tmp/ohosHttp-v1.3.1
   sudo sh install.sh
   ```

3. 验证安装（新终端或 source 后）：
   ```
   ohosHttp --version
   ```

## 安装路径

默认安装到: `~/Documents/ohosHttp`

如需自定义路径：
```
sudo sh install.sh /your/custom/path
```

## 卸载

```
sudo sh uninstall.sh
```

## 手动配置环境变量

如果自动写入环境变量失败，请手动添加到 ~/.bashrc：

```bash
# >>> ohosHttp PATH
export PATH="$PATH:${HOME}/Documents/ohosHttp"
# <<< ohosHttp PATH
```

然后执行 `source ~/.bashrc` 使其生效。
