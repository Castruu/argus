import Carbon.HIToolbox
import Foundation

@main struct PinTests {
    @MainActor static func main() {
        let store = Store(sample: [])
        let escape = UInt32(kVK_Escape)
        assert(hotKeyActions[escape] == nil && store.outsideClicks == nil)

        store.pinned = true
        assert(store.outsideClicks != nil, "A pinned panel watches for clicks outside it")
        hotKeyActions[escape]?()
        assert(!store.pinned, "Escape closes a pinned panel")
        assert(store.escKey == nil && store.outsideClicks == nil, "Unpinning gives Escape and the mouse back")

        store.jumpToNext() // nothing needs you: the shortcut shows the panel instead
        assert(store.pinned)
        store.pinned = false
        assert(store.escKey == nil && store.outsideClicks == nil)
        print("Pin tests passed")
    }
}
