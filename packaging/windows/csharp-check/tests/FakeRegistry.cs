// In-memory stand-in for Microsoft.Win32.RegistryKey, so the registration/cleanup code extracted from
// ../../PreviewHandler.cs can run on any OS (see ../README.md). It enforces the rules of the real
// API that matter: paths are case-insensitive, writes need a writable handle, DeleteSubKey refuses
// keys with subkeys, a disposed handle or a deleted key can't be used, and it counts open handles
// (a leaked handle would keep a mounted user hive from unloading).
namespace Microsoft.Win32 {
using System; using System.Collections.Generic; using System.Linq; using System.Text;
public enum RegistryHive { ClassesRoot, CurrentUser, LocalMachine, Users }
public enum RegistryValueKind { String, DWord }
public sealed class Node {
    public string Name; public Node Parent; public bool Deleted;
    public Dictionary<string, Node> Kids = new Dictionary<string, Node>(StringComparer.OrdinalIgnoreCase);
    public Dictionary<string, object> Vals = new Dictionary<string, object>(StringComparer.OrdinalIgnoreCase);
    public string Path { get { return Parent == null ? Name : Parent.Path + "\\" + Name; } }
    public string Dump(string indent) {
        var sb = new StringBuilder();
        foreach (var v in Vals.OrderBy(x => x.Key)) sb.Append(indent + "  " + (v.Key == "" ? "@" : v.Key) + "=" + v.Value + "\n");
        foreach (var k in Kids.Values.OrderBy(x => x.Name)) { sb.Append(indent + "[" + k.Name + "]\n"); sb.Append(k.Dump(indent + "  ")); }
        return sb.ToString();
    }
}
public sealed class RegistryKey : IDisposable {
    public static int OpenHandles;
    readonly Node _n; readonly bool _w; bool _disposed;
    public RegistryKey(Node n, bool writable) { _n = n; _w = writable; OpenHandles++; }
    public static RegistryKey NewRoot(string name) { return new RegistryKey(new Node { Name = name }, true); }
    public Node Node { get { return _n; } }
    void Live() { if (_disposed) throw new ObjectDisposedException(_n.Name); if (_n.Deleted) throw new System.IO.IOException("key marked for deletion: " + _n.Path); }
    void W() { Live(); if (!_w) throw new UnauthorizedAccessException("not writable: " + _n.Path); }
    static string[] Split(string p) { if (p == null) throw new ArgumentNullException(); return p.Split(new[] { '\\' }, StringSplitOptions.RemoveEmptyEntries); }
    Node Find(string p) { Node c = _n; foreach (var s in Split(p)) { if (!c.Kids.TryGetValue(s, out c)) return null; } return c; }
    public string Name { get { return _n.Path; } }
    public RegistryKey OpenSubKey(string p) { return OpenSubKey(p, false); }
    public RegistryKey OpenSubKey(string p, bool writable) { Live(); var f = Find(p); return f == null ? null : new RegistryKey(f, writable); }
    public RegistryKey CreateSubKey(string p) { W(); Node c = _n; foreach (var s in Split(p)) { Node k; if (!c.Kids.TryGetValue(s, out k)) { k = new Node { Name = s, Parent = c }; c.Kids[s] = k; } c = k; } return new RegistryKey(c, true); }
    static void MarkDeleted(Node n) { n.Deleted = true; foreach (var k in n.Kids.Values) MarkDeleted(k); }
    public void DeleteSubKey(string p, bool throwMissing) { W(); var f = Find(p); if (f == null) { if (throwMissing) throw new ArgumentException("missing " + p); return; } if (f.Kids.Count > 0) throw new InvalidOperationException("has subkeys: " + f.Path); f.Parent.Kids.Remove(f.Name); MarkDeleted(f); }
    public void DeleteSubKeyTree(string p, bool throwMissing) { W(); var f = Find(p); if (f == null) { if (throwMissing) throw new ArgumentException("missing " + p); return; } f.Parent.Kids.Remove(f.Name); MarkDeleted(f); }
    public object GetValue(string name) { Live(); object v; return _n.Vals.TryGetValue(name ?? "", out v) ? v : null; }
    public void SetValue(string name, object v) { W(); _n.Vals[name ?? ""] = v; }
    public void SetValue(string name, object v, RegistryValueKind k) { SetValue(name, v); }
    public void DeleteValue(string name, bool throwMissing) { W(); if (!_n.Vals.Remove(name ?? "") && throwMissing) throw new ArgumentException(name); }
    public string[] GetValueNames() { Live(); return _n.Vals.Keys.ToArray(); }
    public string[] GetSubKeyNames() { Live(); return _n.Kids.Keys.ToArray(); }
    public int SubKeyCount { get { Live(); return _n.Kids.Count; } }
    public int ValueCount { get { Live(); return _n.Vals.Count; } }
    public void Dispose() { if (!_disposed) { _disposed = true; OpenHandles--; } }
}
}
namespace LinkcoPdfPreview {
using System; using Microsoft.Win32;
internal static class PreviewEnvironment {
    public static RegistryKey HKLM = RegistryKey.NewRoot("HKEY_LOCAL_MACHINE"), HKCU = RegistryKey.NewRoot("HKEY_CURRENT_USER"), HKCR = RegistryKey.NewRoot("HKEY_CLASSES_ROOT");
    public static RegistryKey OpenHive(RegistryHive h) {
        RegistryKey r = h == RegistryHive.LocalMachine ? HKLM : h == RegistryHive.CurrentUser ? HKCU : HKCR;
        return new RegistryKey(r.Node, true);
    }
}
}
