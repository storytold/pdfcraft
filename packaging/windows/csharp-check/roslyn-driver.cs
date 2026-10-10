// Compile driver for check.cjs. It runs as a console program inside the .NET 10 WebAssembly runtime
// of @live-codes/blazor-wasm and drives the Roslyn compiler that bundle already has loaded (by
// reflection: the program itself is compiled against the plain BCL) to compile C# the way Windows'
// .NET Framework csc.exe does: C# 5, against the .NET Framework 4.8 reference assemblies.
//
// The job arrives on stdin, one "key=value" per line:
//   src=<name> / ref=<name>        source files and reference assemblies (repeatable), each followed
//   data:<name>=<base64>           by its contents (passed inline: this runtime can't block on I/O)
//   kind=library|winexe|exe   lang=5   name=<assembly name>
// Prints every warning and error like csc does, then "RESULT success=… errors=… warnings=… bytes=…".
using System;
using System.Collections;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection;

class Driver
{
    static Assembly Asm(string name) => AppDomain.CurrentDomain.GetAssemblies().First(a => a.GetName().Name == name);

    static object Fill(MethodBase m, object target, Dictionary<string, object> named)
    {
        var ps = m.GetParameters();
        var args = new object[ps.Length];
        for (int i = 0; i < ps.Length; i++)
        {
            if (named.TryGetValue(ps[i].Name, out var v)) args[i] = v;
            else if (ps[i].HasDefaultValue) args[i] = ps[i].DefaultValue is null && ps[i].ParameterType.IsValueType ? Activator.CreateInstance(ps[i].ParameterType) : ps[i].DefaultValue;
            else if (ps[i].ParameterType.IsValueType) args[i] = Activator.CreateInstance(ps[i].ParameterType);
            else args[i] = null;
        }
        return m is ConstructorInfo c ? c.Invoke(args) : m.Invoke(target, args);
    }

    static MethodBase Pick(IEnumerable<MethodBase> candidates, Dictionary<string, object> named, Func<ParameterInfo[], bool> extra = null) =>
        candidates.Where(m => named.Keys.All(k => m.GetParameters().Any(p => p.Name == k)))
                  .Where(m => m.GetParameters().All(p => named.ContainsKey(p.Name) || p.HasDefaultValue))
                  .Where(m => extra == null || extra(m.GetParameters()))
                  .OrderByDescending(m => m.GetParameters().Length).First();

    static void Main()
    {
        var job = new List<KeyValuePair<string, string>>();
        string line;
        while ((line = Console.ReadLine()) != null)
        {
            int eq = line.IndexOf('=');
            if (eq > 0) job.Add(new KeyValuePair<string, string>(line.Substring(0, eq), line.Substring(eq + 1)));
        }
        string Get(string k, string d = null) => job.Where(kv => kv.Key == k).Select(kv => kv.Value).DefaultIfEmpty(d).First();
        var baseUrl = Get("base");

        var ca = Asm("Microsoft.CodeAnalysis");
        var cs = Asm("Microsoft.CodeAnalysis.CSharp");
        Type T(Assembly a, string n) => a.GetType(n, true);
        var tParseOptions = T(cs, "Microsoft.CodeAnalysis.CSharp.CSharpParseOptions");
        var tLangVer = T(cs, "Microsoft.CodeAnalysis.CSharp.LanguageVersion");
        var tTree = T(cs, "Microsoft.CodeAnalysis.CSharp.CSharpSyntaxTree");
        var tSyntaxTree = T(ca, "Microsoft.CodeAnalysis.SyntaxTree");
        var tMetaRef = T(ca, "Microsoft.CodeAnalysis.MetadataReference");
        var tCompOptions = T(cs, "Microsoft.CodeAnalysis.CSharp.CSharpCompilationOptions");
        var tOutputKind = T(ca, "Microsoft.CodeAnalysis.OutputKind");
        var tOptLevel = T(ca, "Microsoft.CodeAnalysis.OptimizationLevel");
        var tPlatform = T(ca, "Microsoft.CodeAnalysis.Platform");
        var tComp = T(cs, "Microsoft.CodeAnalysis.CSharp.CSharpCompilation");
        var lang = Enum.ToObject(tLangVer, int.Parse(Get("lang", "5")));
        var parseOptions = Fill(Pick(tParseOptions.GetConstructors(), new() { ["languageVersion"] = lang }), null, new() { ["languageVersion"] = lang });

        var trees = new List<object>();
        foreach (var src in job.Where(kv => kv.Key == "src").Select(kv => kv.Value))
        {
            var text = System.Text.Encoding.UTF8.GetString(Convert.FromBase64String(Get("data:" + src)));
            var named = new Dictionary<string, object> { ["text"] = text, ["options"] = parseOptions, ["path"] = src };
            var parse = Pick(tTree.GetMethods(BindingFlags.Public | BindingFlags.Static).Where(m => m.Name == "ParseText"), named, ps => ps[0].ParameterType == typeof(string) && !ps.Any(p => p.Name == "diagnosticOptions" || p.Name == "isGeneratedCode"));
            trees.Add(Fill(parse, null, named));
        }
        var refs = new List<object>();
        var createFromImage = tMetaRef.GetMethods(BindingFlags.Public | BindingFlags.Static)
            .First(m => m.Name == "CreateFromImage" && m.GetParameters()[0].ParameterType == typeof(IEnumerable<byte>));
        foreach (var r in job.Where(kv => kv.Key == "ref").Select(kv => kv.Value))
        {
            var bytes = Convert.FromBase64String(Get("data:" + r));
            refs.Add(Fill(createFromImage, null, new() { ["peImage"] = bytes, ["filePath"] = r }));
        }
        var kind = Get("kind", "library") switch { "winexe" => 1, "exe" => 0, _ => 2 };
        var optNamed = new Dictionary<string, object>
        {
            ["outputKind"] = Enum.ToObject(tOutputKind, kind),
            ["optimizationLevel"] = Enum.ToObject(tOptLevel, 1), // Release
            ["platform"] = Enum.ToObject(tPlatform, 0),          // AnyCpu
            ["warningLevel"] = 4,
            ["concurrentBuild"] = false,
        };
        var compOptions = Fill(Pick(tCompOptions.GetConstructors(), optNamed), null, optNamed);
        var treeArr = Array.CreateInstance(tSyntaxTree, trees.Count);
        for (int i = 0; i < trees.Count; i++) treeArr.SetValue(trees[i], i);
        var refArr = Array.CreateInstance(tMetaRef, refs.Count);
        for (int i = 0; i < refs.Count; i++) refArr.SetValue(refs[i], i);
        var compNamed = new Dictionary<string, object> { ["assemblyName"] = Get("name", "Out"), ["syntaxTrees"] = treeArr, ["references"] = refArr, ["options"] = compOptions };
        var compilation = Fill(Pick(tComp.GetMethods(BindingFlags.Public | BindingFlags.Static).Where(m => m.Name == "Create"), compNamed), null, compNamed);
        var pe = new MemoryStream();
        var emitNamed = new Dictionary<string, object> { ["peStream"] = pe };
        var emit = Pick(tComp.GetMethods().Where(m => m.Name == "Emit" && !m.IsStatic), emitNamed, ps => ps[0].ParameterType == typeof(Stream));
        var result = Fill(emit, compilation, emitNamed);
        var success = (bool)result.GetType().GetProperty("Success").GetValue(result);
        var diags = (IEnumerable)result.GetType().GetProperty("Diagnostics").GetValue(result);
        int errors = 0, warnings = 0;
        foreach (var d in diags)
        {
            var sev = d.GetType().GetProperty("Severity").GetValue(d).ToString();
            if (sev == "Error") errors++; else if (sev == "Warning") warnings++; else continue;
            Console.WriteLine(d.ToString());
        }
        Console.WriteLine($"RESULT success={success} errors={errors} warnings={warnings} bytes={pe.Length}");
    }
}
