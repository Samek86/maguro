// Minimal UI Automation name->centre helper for Maguro promo recording.
// Compiled on first use by rec.py (csc.exe). Not committed as a binary.
using System;
using System.Globalization;
using System.Windows.Automation;

class UiaFind {
    static string Arg(string[] a, string key) {
        for (int i = 0; i < a.Length - 1; i++)
            if (a[i] == key) return a[i + 1];
        return null;
    }
    static bool Has(string[] a, string key) {
        foreach (var x in a) if (x == key) return true;
        return false;
    }
    static int Main(string[] args) {
        var name = Arg(args, "--name");
        if (string.IsNullOrEmpty(name)) {
            Console.Error.WriteLine("need --name");
            return 2;
        }
        var contains = Has(args, "--contains");
        var hwndS = Arg(args, "--hwnd");
        var regionS = Arg(args, "--region"); // l,t,r,b
        var control = Arg(args, "--control"); // optional ControlType name e.g. MenuItem, ListItem
        AutomationElement root = AutomationElement.RootElement;
        if (!string.IsNullOrEmpty(hwndS)) {
            root = AutomationElement.FromHandle((IntPtr)long.Parse(hwndS, CultureInfo.InvariantCulture));
            if (root == null) { Console.Error.WriteLine("bad hwnd"); return 3; }
        }
        Condition cond = Condition.TrueCondition;
        if (!string.IsNullOrEmpty(control)) {
            var ct = (ControlType)typeof(ControlType).GetField(control).GetValue(null);
            cond = new PropertyCondition(AutomationElement.ControlTypeProperty, ct);
        }
        AutomationElementCollection all;
        try { all = root.FindAll(TreeScope.Descendants, cond); }
        catch (Exception ex) { Console.Error.WriteLine("FindAll: " + ex.Message); return 4; }
        int l = int.MinValue, t = int.MinValue, r = int.MaxValue, b = int.MaxValue;
        if (!string.IsNullOrEmpty(regionS)) {
            var p = regionS.Split(',');
            l = int.Parse(p[0], CultureInfo.InvariantCulture);
            t = int.Parse(p[1], CultureInfo.InvariantCulture);
            r = int.Parse(p[2], CultureInfo.InvariantCulture);
            b = int.Parse(p[3], CultureInfo.InvariantCulture);
        }
        AutomationElement best = null;
        System.Windows.Rect bestRc = new System.Windows.Rect();
        int n = 0;
        for (int i = 0; i < all.Count; i++) {
            var el = all[i];
            string nm;
            try { nm = el.Current.Name ?? ""; }
            catch { continue; }
            bool ok = contains
                ? nm.IndexOf(name, StringComparison.OrdinalIgnoreCase) >= 0
                : string.Equals(nm, name, StringComparison.OrdinalIgnoreCase);
            if (!ok) continue;
            System.Windows.Rect rc;
            try { rc = el.Current.BoundingRectangle; }
            catch { continue; }
            if (rc.Width <= 1 || rc.Height <= 1 || double.IsNaN(rc.X)) continue;
            int cx = (int)(rc.X + rc.Width / 2.0);
            int cy = (int)(rc.Y + rc.Height / 2.0);
            if (cx < l || cy < t || cx > r || cy > b) continue;
            n++;
            Console.Error.WriteLine("match {0} {1} {2}x{3} name={4}", cx, cy, (int)rc.Width, (int)rc.Height, nm);
            // Prefer a mid-sized icon (skip huge pane containers that only contain the name).
            if (best == null || (rc.Width * rc.Height < bestRc.Width * bestRc.Height
                                 && rc.Width >= 16 && rc.Height >= 16)) {
                best = el;
                bestRc = rc;
            }
        }
        if (best == null) { Console.Error.WriteLine("not found (n=0)"); return 1; }
        Console.WriteLine("{0} {1} {2} {3}",
            (int)(bestRc.X + bestRc.Width / 2.0),
            (int)(bestRc.Y + bestRc.Height / 2.0),
            (int)bestRc.Width, (int)bestRc.Height);
        return 0;
    }
}
