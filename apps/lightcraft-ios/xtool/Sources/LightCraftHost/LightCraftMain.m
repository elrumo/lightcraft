// The app's entry point. The Rust crate owns the UIApplication (winit calls UIApplicationMain from
// lightcraft_ios_main).
#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#include <stdlib.h>

#include "LightCraftHost.h"

void lightcraft_host_log(const char *line) {
    if (line != NULL) {
        NSLog(@"LightCraft: %s", line);
    }
}

int main(int argc, char *argv[]) {
    @autoreleasepool {
        // the name this device signs in to a sync server with (Settings > Sync, docs/sync.md); since
        // iOS 16 this is the model ("iPhone", "iPad") unless the app has the user-assigned-name entitlement
        NSString *name = UIDevice.currentDevice.name;
        if (name.length > 0) {
            setenv("LIGHTCRAFT_DEVICE", name.UTF8String, 0);
        }
    }
    lightcraft_ios_main();
    return 0;
}
