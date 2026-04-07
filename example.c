// stdin_test.c
// 標準入力のテスト用プログラム

#include <stdio.h>
#include <string.h>

int add(int a, int b) {
    return a + b;
}

int main() {
    printf("=== 標準入力テスト ===\n");

    // 整数入力
    int a, b;
    printf("1つ目の整数を入力してください: ");
    scanf("%d", &a);

    printf("2つ目の整数を入力してください: ");
    scanf("%d", &b);

    printf("合計: %d + %d = %d\n", a, b, add(a, b));

    // 文字列入力
    char name[64];
    printf("あなたの名前を入力してください: ");
    scanf("%s", name);
    printf("こんにちは、%sさん!\n", name);

    // 複数回の入力
    printf("\n=== 簡易計算機 ===\n");
    int result = 0;
    int n;
    printf("何回計算しますか？: ");
    scanf("%d", &n);

    for (int i = 0; i < n; i++) {
        int x;
        printf("%d回目の数値を入力: ", i + 1);
        scanf("%d", &x);
        result += x;
        printf("現在の合計: %d\n", result);
    }

    printf("\n最終結果: %d\n", result);
    printf("終了します。\n");

    return 0;
}