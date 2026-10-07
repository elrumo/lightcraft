#import <UIKit/UIKit.h>
#import <objc/runtime.h>

#include "LightCraftHost.h"

// iOS 27 traps apps that don't adopt the scene lifecycle, and winit 0.30/0.31 has no scene support:
// it creates its window with -[UIWindow initWithFrame:], which a scene-based app never displays.
// So the host (1) declares scene adoption with this empty scene delegate and (2) makes
// -[UIWindow initWithFrame:] create the window in the connected window scene instead.
@interface LCSceneDelegate : UIResponder <UIWindowSceneDelegate>
@property(nonatomic, strong) UIWindow *window;
@end

@implementation LCSceneDelegate
@end

// winit's view (WinitUIView) takes typing through UIKeyInput's -insertText:, where the keyboard's
// Return arrives as a "\n" that egui drops (not printable, and no key). The app is told first and
// presses Enter for egui (docs/ios-gaps.md, A1.9); winit still gets the text, which wakes a frame.
static void (*lc_insert_text_original)(id, SEL, NSString *);

static void lc_insert_text(id self, SEL cmd, NSString *text) {
    if ([text isEqualToString:@"\n"]) {
        lightcraft_host_return_key();
    }
    lc_insert_text_original(self, cmd, text);
}

// Called by every window's -initWithFrame: (below) until winit has registered its view class; winit
// creates its view before its window. Main thread only, as UIWindow is.
static void lc_hook_return_key(void) {
    static BOOL hooked = NO;
    if (hooked) {
        return;
    }
    Class view = NSClassFromString(@"WinitUIView");
    Method m = view != Nil ? class_getInstanceMethod(view, @selector(insertText:)) : NULL;
    if (m != NULL) {
        lc_insert_text_original = (void (*)(id, SEL, NSString *))method_setImplementation(m, (IMP)lc_insert_text);
        hooked = YES;
    }
}

@implementation UIWindow (LCScene)

+ (void)load {
    Method a = class_getInstanceMethod(self, @selector(initWithFrame:));
    Method b = class_getInstanceMethod(self, @selector(initLCWithFrame:));
    method_exchangeImplementations(a, b);
}

- (instancetype)initLCWithFrame:(CGRect)frame {
    lc_hook_return_key();
    for (UIScene *s in UIApplication.sharedApplication.connectedScenes) {
        if ([s isKindOfClass:[UIWindowScene class]]) {
            self = [self initWithWindowScene:(UIWindowScene *)s]; // not recursive: initWithWindowScene doesn't call initWithFrame:
            self.frame = frame;
            return self;
        }
    }
    return [self initLCWithFrame:frame]; // the original, after the exchange
}

@end
