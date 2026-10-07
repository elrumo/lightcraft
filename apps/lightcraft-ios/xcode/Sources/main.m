// The Rust crate owns the UIApplication (winit calls UIApplicationMain from lightcraft_ios_main).
extern void lightcraft_ios_main(void);

int main(int argc, char *argv[]) {
    lightcraft_ios_main();
    return 0;
}
