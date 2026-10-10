// Uninstall cleanup of Linkco PDF Editor's per-user Explorer registration, run against the real code
// of ../../PreviewHandler.cs (RegRoot and the LinkcoPdfPreviewHandler registry helpers, extracted
// by check.cjs) on the in-memory registry of FakeRegistry.cs. Prints "ALL PASS" when every check holds.
namespace LinkcoPdfPreview {
using System; using Microsoft.Win32;
internal static partial class LinkcoPdfPreviewHandler {}
public static class Program {
    const string P = "{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}", T = "{3D8CDE4B-E969-481F-BEB0-5E3B98287416}";
    const string PV = "{8895b1c6-b41f-4c1c-a562-0d564250836f}", TH = "{e357fccd-a995-4576-b01f-234630154e96}";
    const string OTHER = "{0D3C1E7A-5B9F-4C2D-8E61-7F4A2B9C3D10}", ACRO = "{DC6EFB56-9CFA-464D-8880-44885D7DC193}";
    static int fails;
    static void Check(bool ok, string what) { Console.WriteLine((ok ? "  ok   " : "  FAIL ") + what); if (!ok) fails++; }
    static string Val(RegistryKey root, string path) { using (var k = root.OpenSubKey(path)) return k == null ? null : k.GetValue("") as string; }
    static bool Has(RegistryKey root, string path) { using (var k = root.OpenSubKey(path)) return k != null; }
    static void Set(RegistryKey root, string path, string name, string v) { using (var k = root.CreateSubKey(path)) k.SetValue(name, v); }

    // A user's registry as the app (schema 4) leaves it, Classes in `cl`, the rest in `sw`.
    static void Populate(RegistryKey sw, RegistryKey cl, string clsPrefix) {
        Set(cl, clsPrefix + "CLSID\\" + P + "\\InprocServer32", "CodeBase", "file:///C:/Gone/LinkcoPdfPreviewHandler.dll");
        Set(cl, clsPrefix + "CLSID\\" + T + "\\InprocServer32", "CodeBase", "file:///C:/Gone/LinkcoPdfPreviewHandler.dll");
        Set(cl, clsPrefix + "LinkcoPDFEditor.PreviewHandler\\CLSID", "", P);
        Set(cl, clsPrefix + "LinkcoPDFEditor.ThumbnailProvider\\CLSID", "", T);
        Set(cl, clsPrefix + "CLSID\\" + OTHER, "", "Other PDF previewer (per user)");
        Set(cl, clsPrefix + ".pdf\\ShellEx\\" + PV, "", P);
        Set(cl, clsPrefix + "SystemFileAssociations\\.pdf\\ShellEx\\" + PV, "", P);
        Set(cl, clsPrefix + "Vendor.PdfFile\\ShellEx\\" + PV, "", P);
        Set(cl, clsPrefix + "Vendor.PdfFile", "", "Vendor PDF");               // a real ProgID: must survive
        Set(cl, clsPrefix + "FormerDefault.Pdf\\ShellEx\\" + PV, "", P);         // old default app, no backup
        Set(cl, clsPrefix + "AcroLike.Document\\ShellEx\\" + PV, "", P);         // backup is machine-wide CLSID
        Set(cl, clsPrefix + "SystemFileAssociations\\.pdf\\ShellEx\\" + TH, "", T);
        Set(cl, clsPrefix + "LinkcoPDFEditor.Document\\ShellEx\\" + TH, "", T);
        Set(cl, clsPrefix + "Unrelated.Txt\\ShellEx\\" + PV, "", OTHER);       // not ours: untouched
        Set(sw, "Software\\Microsoft\\Windows\\CurrentVersion\\PreviewHandlers", P, "Linkco PDF Preview Handler");
        Set(sw, "Software\\Microsoft\\Windows\\CurrentVersion\\PreviewHandlers", OTHER, "Other");
        Set(sw, "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.pdf\\UserChoice", "ProgId", "FormerDefault.Pdf");
        string cfg = "Software\\Linkco\\Linkco PDF Editor";
        Set(sw, cfg, "PreviewHandlerDll", "C:\\Gone\\LinkcoPdfPreviewHandler.dll");
        Set(sw, cfg, "PreviewHandlerDllStamp", "1:2");
        Set(sw, cfg, "CliPath", "C:\\Gone\\pdfcraft-cli.exe");
        Set(sw, cfg, "PreviousPdfPreviewHandler", OTHER);
        Set(sw, cfg, "PreviousSysPdfPreviewHandler", "{99999999-0000-0000-0000-000000000000}");  // gone app: removed, not restored
        Set(sw, cfg, "PrevProgId_Vendor.PdfFile", OTHER);
        Set(sw, cfg, "PrevProgId_AcroLike.Document", ACRO);
    }

    static void Verify(RegistryKey sw, RegistryKey cl, string c, string label) {
        Console.WriteLine(label);
        Check(!Has(cl, c + "CLSID\\" + P) && !Has(cl, c + "CLSID\\" + T), "our COM servers removed");
        Check(!Has(cl, c + "LinkcoPDFEditor.PreviewHandler") && !Has(cl, c + "LinkcoPDFEditor.ThumbnailProvider"), "our ProgIDs removed");
        Check(Has(cl, c + "CLSID\\" + OTHER), "other app's CLSID kept");
        Check(Val(cl, c + ".pdf\\ShellEx\\" + PV) == OTHER, ".pdf preview handler restored (per-user CLSID)");
        Check(!Has(cl, c + "SystemFileAssociations\\.pdf\\ShellEx\\" + PV), "SFA preview: backup app gone -> removed");
        Check(Val(cl, c + "Vendor.PdfFile\\ShellEx\\" + PV) == OTHER && Val(cl, c + "Vendor.PdfFile") == "Vendor PDF", "vendor ProgID restored, ProgID kept");
        Check(Val(cl, c + "AcroLike.Document\\ShellEx\\" + PV) == ACRO, "ProgID restored to machine-wide CLSID");
        Check(!Has(cl, c + "FormerDefault.Pdf"), "ProgID that only pointed at us removed (UserChoice/sweep)");
        Check(!Has(cl, c + "SystemFileAssociations\\.pdf\\ShellEx\\" + TH) && !Has(cl, c + "LinkcoPDFEditor.Document"), "thumbnail registrations removed");
        Check(Val(cl, c + "Unrelated.Txt\\ShellEx\\" + PV) == OTHER, "unrelated ProgID untouched");
        using (var h = sw.OpenSubKey("Software\\Microsoft\\Windows\\CurrentVersion\\PreviewHandlers"))
            Check(h.GetValue(P) == null && h.GetValue(OTHER) != null, "PreviewHandlers: ours removed, other kept");
        using (var cfg = sw.OpenSubKey("Software\\Linkco\\Linkco PDF Editor")) {
            bool any = false; foreach (var n in cfg.GetValueNames()) if (n.StartsWith("Prev")) any = true;
            Check(!any, "config: registration values and backups removed");
        }
    }

    public static void Main() {
        // Machine: Acrobat-like CLSID registered machine-wide; HKCR .pdf default is some app.
        Set(PreviewEnvironment.HKLM, "Software\\Classes\\CLSID\\" + ACRO, "", "Acrobat-like previewer");
        Set(PreviewEnvironment.HKCR, ".pdf", "", "Acrobat.Document.DC");

        // 1. Another user, split hives (NTUSER.DAT + UsrClass.dat).
        var sw = RegistryKey.NewRoot("HKEY_USERS\\Mount"); var cl = RegistryKey.NewRoot("HKEY_USERS\\Mount_Classes");
        Populate(sw, cl, "");
        int before = RegistryKey.OpenHandles;
        Check(LinkcoPdfPreviewHandler.CleanUserHive(sw, cl), "split: registration found and cleaned");
        Check(RegistryKey.OpenHandles == before, "split: no registry handles leaked (hive can be unmounted) " + (RegistryKey.OpenHandles - before));
        Verify(sw, cl, "", "split hives:");
        Check(!Has(sw, "Software\\Classes"), "split: nothing written to Software\\Classes of NTUSER.DAT");
        string snap = sw.Node.Dump("") + cl.Node.Dump("");
        Check(!LinkcoPdfPreviewHandler.CleanUserHive(sw, cl), "split: second run finds nothing");
        Check(snap == sw.Node.Dump("") + cl.Node.Dump(""), "split: second run changes nothing");

        // 2. Same user layout in a single hive (logged-on user: HKU\SID with Software\Classes).
        var one = RegistryKey.NewRoot("HKEY_USERS\\S-1-5-21-1-2-3-1001");
        Populate(one, one, "Software\\Classes\\");
        Check(LinkcoPdfPreviewHandler.CleanUserHive(one, null), "single: registration found and cleaned");
        Verify(one, one, "Software\\Classes\\", "single hive:");

        // 3. A user who never ran the app: untouched, byte for byte.
        var sw3 = RegistryKey.NewRoot("HKEY_USERS\\M3"); var cl3 = RegistryKey.NewRoot("HKEY_USERS\\M3_Classes");
        Set(cl3, ".pdf\\ShellEx\\" + PV, "", ACRO); Set(cl3, "Acro.Pdf\\ShellEx\\" + PV, "", ACRO);
        Set(sw3, "Software\\Linkco\\Linkco PDF Editor", "CliPath", "x"); Set(sw3, "Software\\Other", "a", "b");
        string s3 = sw3.Node.Dump("") + cl3.Node.Dump("");
        Check(!LinkcoPdfPreviewHandler.CleanUserHive(sw3, cl3), "untouched user: nothing found");
        Check(s3 == sw3.Node.Dump("") + cl3.Node.Dump(""), "untouched user: hive unchanged");

        // 4. Only a stale ProgID pointing at us (no CLSID, no config) is still found.
        var sw4 = RegistryKey.NewRoot("HKEY_USERS\\M4"); var cl4 = RegistryKey.NewRoot("HKEY_USERS\\M4_Classes");
        Set(cl4, "Stale.Pdf\\ShellEx\\" + PV, "", P);
        Check(LinkcoPdfPreviewHandler.CleanUserHive(sw4, cl4) && !Has(cl4, "Stale.Pdf"), "stale ProgID only: found and removed");

        // 5. RegRoot routing edge cases.
        var r = new RegRoot(sw4, cl4);
        using (var k = r.CreateSubKey("Software\\ClassesX\\A")) { }
        Check(Has(sw4, "Software\\ClassesX\\A") && !Has(cl4, "X\\A"), "routing: 'Software\\ClassesX' is not the classes hive");
        using (var k = r.CreateSubKey("software\\classes\\Lower")) { }
        Check(Has(cl4, "Lower"), "routing: case-insensitive");

        // 6. DllPathKey: the same vectors as windows_preview.rs's dll_path_key test (the app compares
        //    this ASCII form through reg.exe, which can't print most non-ASCII paths).
        Console.WriteLine("DllPathKey:");
        Check(LinkcoPdfPreviewHandler.DllPathKey("C:\\Program Files\\Linkco\\Linkco PDF Editor\\LinkcoPdfPreviewHandler.dll") ==
            "C:\\PROGRAM FILES\\LINKCO\\LINKCO PDF EDITOR\\LINKCOPDFPREVIEWHANDLER.DLL", "ASCII path: upper-cased");
        Check(LinkcoPdfPreviewHandler.DllPathKey("C:\\Users\\\u0645\u062d\u0645\u062f\\AppData\\Local\\Programs\\Linkco\\Linkco PDF Editor\\LinkcoPdfPreviewHandler.dll") ==
            "C:\\USERS\\%0645%062D%0645%062F\\APPDATA\\LOCAL\\PROGRAMS\\LINKCO\\LINKCO PDF EDITOR\\LINKCOPDFPREVIEWHANDLER.DLL", "Arabic user name: %XXXX");
        Check(LinkcoPdfPreviewHandler.DllPathKey("D:\\100% Tools\\\u00dcn\u00efcode \U0001F600\\x.dll") ==
            "D:\\100%0025 TOOLS\\%00DCN%00EFCODE %D83D%DE00\\X.DLL", "'%', Latin-1 and a surrogate pair");

        Console.WriteLine(fails == 0 ? "ALL PASS" : ("FAILURES: " + fails));
    }
}
}
