# 定义编译器
CC = musl-gcc

# 定义编译选项：-static 确保生成静态程序
CFLAGS = -Wall -O2 -static

# 目标文件名
TARGET = helloworld

all: $(TARGET)

$(TARGET): main.c
	$(CC) $(CFLAGS) -o $(TARGET) main.c

clean:
	rm -f $(TARGET)
