#!/usr/bin/env python3
# -*- coding: utf-8 -*-

import sys

def process_line(line):
    """
    处理单行文本：
    1. 去除末尾换行符（保留原始换行状态）
    2. 删除前 6 个字符（如果行长度不足 6，则变为空）
    3. 转为大写
    4. 恢复换行符
    """
    # 判断是否以换行符结尾，用于后续恢复
    has_newline = line.endswith('\n')
    content = line.rstrip('\n')  # 去掉换行符

    # 删除前 6 个字符
    if len(content) >= 6:
        content = content[6:]
    else:
        content = ""

    # 转为大写
    content = content.upper()

    # 恢复换行符（若原行有）
    if has_newline:
        content += '\n'
    return content

def main():
    # 如果有命令行参数，则视为输入文件名；否则从标准输入读取
    if len(sys.argv) > 1:
        input_file = sys.argv[1]
        with open(input_file, 'r', encoding='utf-8') as f:
            for line in f:
                sys.stdout.write(process_line(line))
    else:
        for line in sys.stdin:
            sys.stdout.write(process_line(line))

if __name__ == "__main__":
    main()
