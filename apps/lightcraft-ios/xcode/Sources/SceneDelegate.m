#import <UIKit/UIKit.h>
#import <objc/runtime.h>

// iOS 27 traps apps that don't adopt the scene lifecycle, and winit 0.30/0.31 has no scene support:
// it creates its window with -[UIWindow initWithFrame:], which a scene-based app never displays.
// So the host (1) declares scene adoption with this empty scene delegate and (2) makes
// -[UIWindow initWithFrame:] create the window in the connected window scene instead.
@interface LCSceneDelegate : UIResponder <UIWindowSceneDelegate>
@property(nonatomic, strong) UIWindow *window;
@end

@implementation LCSceneDelegate
@end

@implementation UIWindow (LCScene)

+ (void)load {
    Method a = class_getInstanceMethod(self, @selector(initWithFrame:));
    Method b = class_getInstanceMethod(self, @selector(initLCWithFrame:));
    method_exchangeImplementations(a, b);
}

- (instancetype)initLCWithFrame:(CGRect)frame {
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
