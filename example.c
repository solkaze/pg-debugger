// scroll_test.c
// ソースビューとコンソールのスクロールテスト用プログラム

#include <stdio.h>

// --- ユーティリティ関数 ---

int add(int a, int b) {
    return a + b;
}

int subtract(int a, int b) {
    return a - b;
}

int multiply(int a, int b) {
    return a * b;
}

int factorial(int n) {
    if (n <= 1) {
        return 1;
    }
    return n * factorial(n - 1);
}

int fibonacci(int n) {
    if (n <= 0) return 0;
    if (n == 1) return 1;
    int a = 0, b = 1, c;
    for (int i = 2; i <= n; i++) {
        c = a + b;
        a = b;
        b = c;
    }
    return b;
}

// --- 配列操作 ---

void print_array(int arr[], int size) {
    printf("[");
    for (int i = 0; i < size; i++) {
        printf("%d", arr[i]);
        if (i < size - 1) printf(", ");
    }
    printf("]\n");
}

void bubble_sort(int arr[], int size) {
    for (int i = 0; i < size - 1; i++) {
        for (int j = 0; j < size - i - 1; j++) {
            if (arr[j] > arr[j + 1]) {
                int tmp = arr[j];
                arr[j]     = arr[j + 1];
                arr[j + 1] = tmp;
            }
        }
    }
}

int sum_array(int arr[], int size) {
    int total = 0;
    for (int i = 0; i < size; i++) {
        total += arr[i];
    }
    return total;
}

// --- メイン処理 ---

int main() {
    printf("=== 四則演算 ===\n");
    int x = 10;
    int y = 3;
    printf("x = %d, y = %d\n", x, y);
    printf("add      : %d\n", add(x, y));
    printf("subtract : %d\n", subtract(x, y));
    printf("multiply : %d\n", multiply(x, y));

    printf("\n=== 階乗 ===\n");
    for (int i = 1; i <= 8; i++) {
        printf("%d! = %d\n", i, factorial(i));
    }

    printf("\n=== フィボナッチ数列 ===\n");
    for (int i = 0; i <= 10; i++) {
        printf("fib(%2d) = %d\n", i, fibonacci(i));
    }

    printf("\n=== バブルソート ===\n");
    int arr[] = {64, 34, 25, 12, 22, 11, 90};
    int size = sizeof(arr) / sizeof(arr[0]);
    printf("ソート前: ");
    print_array(arr, size);
    bubble_sort(arr, size);
    printf("ソート後: ");
    print_array(arr, size);
    printf("合計: %d\n", sum_array(arr, size));

    printf("\n=== 九九 ===\n");
    for (int i = 1; i <= 9; i++) {
        for (int j = 1; j <= 9; j++) {
            printf("%3d", i * j);
        }
        printf("\n");
    }

    printf("\n完了!\n");
    return 0;
}