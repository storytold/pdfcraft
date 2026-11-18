# TR-08 — Windows fiziksel klavye ve pano kabulü

Bu liste, Windows'ta **gerçek tuş basışlarıyla** Türkçe Q ve F girişini, sistem
panosunu ve Türkçe dosya yollarında kaydetme/yeniden açmayı doğrulamak içindir.
Her Windows klavye düzeni için ayrı kayıt doldurun. Yazılım tarafından üretilen
metin veya sanal tuş eşlemesi, fiziksel klavye kabulü yerine geçmez.

Bu yönerge TR-08 fiziksel giriş kabulünü kapsar. Derlemeyi
hazırlamak için [Windows geliştirme yönergesini](windows-development-build.md)
izleyin. Bu kontrol, temiz kurulum, yüksek DPI, yazdırma veya Windows 1.0 yayın
kabulünün tamamı değildir.

## Çalıştırılabilir koşucu ve kabul sınırı

`packaging/windows/test-turkish-input.ps1`, PowerShell 7.4+ üzerinde çalışan
**salt okunur preflight ve sentetik durum denetimleri** sağlar. Varsayılanı,
`-PreflightOnly` ve eşanlamlı `-ValidateInputs` aynı işlemdir. EXE çalıştırmaz,
profil/çıktı dizini oluşturmaz, ayarları değiştirmez ve OS panosuna erişmez.
`-GuiTest` ayrılmış bir opt-in'dir; bu sürümde canlı GUI taşıyıcısı yoktur ve
**REFUSE / exit 3** döner. Preflight'ın geçmesi native kabul değildir.

Önce güncel kaynak adayı için uygulama ve CLI'yi
[Windows geliştirme akışıyla](windows-development-build.md) hazırlayın. Taze
derlemenin kaynak kaydını ayrıca tutun; dosya hash'i tek başına derlemenin
kaynakla güncelliğini veya çalışabilirliğini kanıtlamaz. Eski rapor hash'lerini
yeni adaya taşımayın. Parametreler her makinede seçilebilir; örnek yollar
kişisel ayar veya belge klasörü değildir:

```powershell
$bin = './target/x86_64-pc-windows-msvc/debug'
$fixtureRoot = './synthetic-input-acceptance'
$fixture = Join-Path $fixtureRoot 'Kaynak - İş.pdf'
# Root ve PDF önceden yalnız izinli varlıklardan hazırlanmış sentetik girdilerdir.
# RunDirectory ve altındaki profil/çıktılar henüz var olmamalıdır.
$preflight = @{
    AppExecutable = (Join-Path $bin 'pdfcraft.exe')
    CliExecutable = (Join-Path $bin 'pdfcraft-cli.exe')
    ExpectedAppSha256 = '<güncel adayın kaydedilmiş 64 haneli SHA-256 değeri>'
    ExpectedCliSha256 = '<güncel adayın kaydedilmiş 64 haneli SHA-256 değeri>'
    FixtureRoot = $fixtureRoot
    FixturePdf = $fixture
    ExpectedFixtureSha256 = '<sentetik PDF için kaydedilmiş 64 haneli SHA-256>'
    RunDirectory = (Join-Path $fixtureRoot 'run-001')
    SyntheticFixturesOnly = $true
}
./packaging/windows/test-turkish-input.ps1 @preflight -PreflightOnly
```

`SyntheticFixturesOnly`, operatörün açık sentetik girdi beyanıdır; PDF'in içeriği
otomatik denetlenmez. Üç hash pini zorunludur. Aynı EXE/CLI yolu, kök dışı ya da
kökün kendisini gösteren çıktı, mevcut run (boş olsa bile), mevcut profil ve
yolun herhangi bir üstünde symlink/junction/reparse noktası reddedilir.
`RunDirectory` yalnız fixture kökünün altında olabilir. Windows `\\?\` ve
`\\?\UNC\` önekleri yalnız yol karşılaştırmasında ele alınır; Unicode
normalleştirilmez. Aygıt/alternate stream yolları kabul edilmez.

Koşucu JSON özeti kişisel yol, editör değeri veya kontrol token'ı içermez.
Dosya yazmadan stdout'a döner:

| Exit | Durum | Anlamı |
|---|---|---|
| 0 | `VALIDATED_INPUTS` | Salt okunur girdi kontrolleri geçti; GUI/klavye/pano NOT VERIFIED |
| 2 | `INVALID_INPUTS` | Eksik/yanlış pin, yol, mevcut çıktı/profil veya başka doğrulama hatası |
| 3 | `REFUSE` | `-GuiTest`: bu sürümde canlı taşıyıcı etkin değil |

PowerShell'in başlangıç/parametre çözümleme hataları JSON sözleşmesinden önce
oluşabilir; sıfır dışı her çıkış başarısızlıktır. Test betiği kendi başarısız
assertion'larında exit 1 döner.

### Bağımlılıksız fixture/mock regresyonu

```powershell
pwsh -NoProfile -File ./packaging/windows/test-turkish-input.Tests.ps1
# İsteğe bağlı özel kanıt çıktısı:
pwsh -NoProfile -File ./packaging/windows/test-turkish-input.Tests.ps1 `
  -ReportPath ./input-fixture-report.json
```

Pester veya başka araç kurulmaz. Testler geçici GUID klasöründe çalıştırılmayan
sahte EXE baytları, açılmayan sentetik PDF baytları ve mock UI durumları kullanır;
kendi geçici klasörlerini güvenli hedef kontrolünden sonra temizler. Junction
testi gerçek Windows dosya sistemi bağlantısını yalnız bu geçici klasörde
oluşturur ve önce bağlantıyı kaldırır. Testler uygulama, tarayıcı, OS panosu,
klavye düzeni veya yazıcı üzerinde işlem yapmaz.

| Envanter | Çalıştırılabilir durum | Gerçek kabul |
|---|---|---|
| CLI varsayılan / `-PreflightOnly` / `-ValidateInputs` | Hazır; hash/yol/profil tazeliği denetlenir | Girdi hazırlığı; native kabul değil |
| Guard fixture/mock testleri | 50 PASS / 0 FAIL; belge kimliği/sayfa sayısı RED → GREEN ile sınandı | Yalnız bu betiklerin sentetik davranışı |
| EventText / egui Paste / IME mock kayıt döngüleri | Kanalı ayrı kaydeden 7 aşamalı durum denetimi | Programatik model; fiziksel/OS pano kanıtı değil |
| `-GuiTest` | Bilerek REFUSE, exit 3 | Bu sürümde native koşucu yok; NOT VERIFIED |
| OS Copy/Cut/Paste | Kayıpsız yedek/geri yükleme sağlayıcısı yok; REFUSE | NFD OS panosu NOT VERIFIED |
| Fiziksel Q/F, temiz Windows | Bu koşucuyla çalıştırılmaz | NOT VERIFIED |

### İlerideki opt-in GUI taşıyıcısının zorunlu sözleşmesi

Canlı taşıyıcı eklenmeden önce şu koşulların tamamı gerçek akışta uygulanıp
yeniden test edilmelidir. Aşağıdaki kütüphane guard'ları bu tur yalnız mock ile
çalışır; bir hazır-dosyası veya kullanıcı tarafından verilen boolean gerçek
foreground/çıkış kanıtı sayılmaz:

1. Taze iki binary, hash pinleri ve yeni run yeniden doğrulanır. Yalnız binary'ler
   `profile/bin` altına kopyalanır; hash'leri tekrar ölçülür. `portable.txt` ve
   yazılabilir `PdfCraftData` yalnız orada oluşturulur. Eski ayar/imza/recovery
   okunmaz veya kopyalanmaz; portable profil yazılamıyorsa uygulama başlatılmaz.
2. Kontrol dosyası yalnız sahibi okuyabilecek izinle, token günlüğe alınmadan
   tutulur. Gerçek foreground PID her mutasyon öncesi sahip olunan uygulamayla
   eşleşir (`Assert-PdfCraftInputForeground`). Hiçbir varsayılan GUI başlatmaz.
3. Her asenkron open/save/undo/reopen bekleyişi tek doğru active path, belge
   kimliği, tek sayfa, görünür sayfa ve beklenen boolean dirty değerini denetler
   (`Wait-PdfCraftInputDocument`). Başka aktif/pending yol derhal reddedilir;
   bekleme süresi dolması başarı sayılmaz.
4. `Assert-PdfCraftInputEditor` aynı benzersiz editor ID, rol, enabled/focus ve
   **Ordinal birebir özgün Unicode** ister. CRLF/LF farkı bu guard'da da eşit
   sayılmaz; gerekiyorsa ayrı kanal gözlemi olarak kaydedilir.
5. Commit → dirty, undo → clean, redo → dirty, save → clean, kapat/reopen → clean
   ve yeniden açılmış değer ayrı doğrulanır. `Test-PdfCraftInputMockLifecycle`
   bu yedi aşamanın yalnız sentetik kayıtlarını sınar; disk kaydını kanıtlamaz.
6. Scalar sınırını aşan NFD'nin tamamı taslakta kalır. Apply kapalı, Enter sonrası
   belge temiz, Cancel sonrası eski değer aynı olmalıdır
   (`Assert-PdfCraftInputRejectedDraft`). Kesilmiş taslak kabul edilmez.
7. Kaynak fixture hash'i ve taze binary hash'leri sonunda değişmemiş olmalıdır.
   `Assert-PdfCraftInputNormalExit`, yalnız sahip olunan prosesin gerçekten
   çıkıp exit 0 verdiği gözlemi kabul eder. Quit cevabı, zorla kapatma veya
   tamamlanmamış kontroller başarı sayılmaz; kirli belge sessizce atılmaz.

### Pano için kapalı varsayılan

`Get-PdfCraftInputClipboardDecision` yalnız çağıranın verdiği `format`, `type`,
`count` metadata nesnelerini sınıflar; gerçek panodan veri/metadata okumaz ve
içerik döndürmez. Opaque/custom/stream formatları, sadece bilinen String
formatları ve boş metadata dahil **her durumda REFUSE** döner. `lossless=true`
gibi bir alan izin sağlamaz. Metadata, kayıpsız backup/restore kanıtı değildir.

Gerçek Copy/Cut/Paste için bütün formatları, ertelenmiş/opaque veriyi ve aynı
baytları kayıpsız geri yükleyip bağımsız doğrulayabilen, ayrıca incelenmiş bir
sağlayıcı gerekir. Pano yarışları ve geri yükleme başarısızlığı da ele alınmalıdır.
Bu sağlayıcı yoksa ilk write'tan önce durulur; kopyala ve kes de panoya yazmadır.
Kullanıcının mevcut panosunu metne indirgemek, boşaltmak veya sadece okunabilen
formatları geri koymak kabul edilmez. Kişisel içerik rapora/görüntüye alınmaz.

## Önceki kanıt ve açık kalan kabul

14–15 Kasım 2026'daki kayıtların kapsamı aşağıdadır. Bunlar kendi
kaynak/binary'lerinin geçmiş kanıtıdır; yeni koşucu testlerinin veya yeni
entegrasyon çıktısının native kabulü değildir.

| Kanıt | Sonuç | Kanıtlamadığı alan |
|---|---|---|
| Gerçek Windows panosu: yeni metin, mevcut paragraf, form, metin kutusu, Fill & Sign metni | 5/5 PASS; kopyala/kes/yapıştır, geri al/yinele, kaydet/yeniden aç | Fiziksel tuş basışı |
| Pano koruması | Önceki 7 biçim ve içerikleri geri yüklenip doğrulandı; kişisel içerik günlüğe yazılmadı | Her başka pano biçiminin otomatik korunması |
| Kurulu Türkçe Q düzeninin Windows API eşlemesi | `çÇğĞıIiİöÖşŞüÜ` için 14/14 PASS | Fiziksel Q girişi, `âÂîÎûÛ` tuş bileşimleri veya F girişi |
| Fiziksel Q / fiziksel F girişi | NOT VERIFIED | Henüz kabul verilmedi |
| Türkçe F düzeni | Test makinesinde kurulu değildi; NOT VERIFIED | Uygulama hatası veya F desteğinin geçtiği sonucu çıkarılmaz |
| Altıncı tur: yeni/mevcut metin, form, textbox, Fill & Sign NFD | EventText ile 5/5 native save/undo/reopen | OS Copy/Cut/Paste, fiziksel tuş, bütün Unicode/PDF'ler |
| Yedinci tur: NFD imza/paraf | EventText ile 2/2 native ve 6/6 ret/Cancel; izole tercihlerde birebir değer | İmza PDF'inden mantıksal metin çıkarımı veya OS panosu |
| NFD OS panosu | Opaque/okunamayan başlangıç formatı nedeniyle yazmadan duruldu; NOT VERIFIED | Önceki NFC pano turu NFD kabulüne taşınmaz |

Fill & Sign'ın çok satırlı pano örneğinde Windows CRLF satır sonları LF oldu.
Satırlar ve Türkçe karakterler korundu; bu vaka bayt düzeyinde aynı pano dizgesi
olarak sayılmadı. Aşağıdaki tek satırlı karşılaştırmada harf, boşluk veya harf
büyüklüğü dönüşümüne izin verilmez. Çok satırlı karşılaştırmada yalnız CRLF/LF
farkı ayrıca kaydedilebilir.

## Test kaydı

Boş alanları doldurun. Aşağıdaki manuel sonuç hücreleri, test gerçekten
çalıştırılana kadar **NOT VERIFIED** kalır.

| Alan | Değer |
|---|---|
| Test tarihi / saat dilimi | |
| Test kaydı kimliği | |
| Uygulama sürümü / derleme etiketi | |
| `pdfcraft.exe` SHA-256 | |
| Kaynak commit'i ve varsa çalışma kopyası değişiklik kaydı | |
| Windows sürümü / mimarisi | |
| Windows girdi düzeni | Türkçe Q / Türkçe F: |
| Fiziksel klavyenin modeli / tuş etiketleri | |
| Uygulama arayüz dili / ekran ölçeği | |
| Kullanılan bağımsız PDF okuyucu / sürümü | |

Kaynak çalışma klasöründen debug dosyasının hash'ini almak için:

```powershell
Get-FileHash ./target/x86_64-pc-windows-msvc/debug/pdfcraft.exe -Algorithm SHA256
```

Farklı bir çıktı klasörü kullanıldıysa gerçek EXE'yi ölçün. Önceki rapordaki
hash'i yeni test kaydına ölçmeden taşımayın.

## Hazırlık

1. Kişisel veri içermeyen sentetik test belgeleri kullanın. Mevcut bir belgeyi
   deneyecekseniz özgününü koruyup ayrı kabul kopyasında çalışın.
2. Pano adımlarını yalnız yukarıdaki kayıpsız backup/restore önkoşulu sağlanırsa
   yürütün. Bu koşucu sağlayıcı içermediğinden pano adımları REFUSE kalır.
   Kişisel pano içeriğini belgeye, ekran görüntüsüne veya hata kaydına almayın.
3. Windows'un girdi göstergesinden Q ya da F düzenini seçin ve kayda yazın.
   Düzen listede yoksa o düzenin satırlarını NOT VERIFIED bırakın; başka bir
   düzenin sonucunu buraya kopyalamayın.
4. Şu göreli test klasörünü oluşturun: `Kabul - Çalışmalar`. Her araç için ayrı
   çıktı kullanın; örneğin `İş sözleşmesi – Şubat – Form.pdf`.
5. Caps Lock başlangıç durumunu kaydedin. Uygulamayı açın; yalnız test belgeleri
   üzerinde çalıştığınızı doğrulayın.

## Referans metin

İlk satırı **panodan yapıştırmadan, fiziksel klavyeyle** yazın. Boşluklar da
örnekteki gibi olmalıdır:

```text
ç Ç ğ Ğ ı I i İ ö Ö ş Ş ü Ü â Â î Î û Û
```

Çok satırlı metin destekleyen araçlarda ikinci örneği de deneyin:

```text
İstanbul Iğdır Şişli
Çağrı Öğrenci GÜNEŞ ışık
İŞ SÖZLEŞMESİ
```

`I/ı` ve `İ/i` çiftlerini Shift ve Caps Lock ile ayrı kontrol edin. `â/î/û`
karakterlerini seçili düzenin yerel tuş bileşimiyle üretin; kullanılan tuşları
bulgu kaydına yazın. Karakteri karakter tablosundan veya panodan almak, fiziksel
giriş satırını geçirmez.

## Her araçta uygulanacak akış

Önce aşağıdaki araç tablosundan düzenleyiciyi açın; ardından bu adımların
tamamını o araçta uygulayın.

1. Tek satırlı referansı fiziksel klavyeyle yazın. Her harfi ve işaretini
   kontrol edin; `?`, boş kutu, eksik nokta/aksan veya komşu harfin kesilmesi FAIL'dir.
2. Sona bir `ğ` yazın, Backspace ile silin. Yalnız eklediğiniz harf silinmeli;
   önceki metin aynı kalmalıdır. Geri al/yinele ile aynı ekleme/silmeyi doğrulayın.
3. Yalnız metin düzenleyicisinin içinde Ctrl+A ve Ctrl+C kullanın. Panodaki
   kopyayı aşağıdaki karşılaştırmayla denetleyin. Bütün belgeyi veya başka bir
   kontrolü seçtiyseniz odağı düzeltip adımı yeniden yapın.
4. Metni Ctrl+X ile kesin. Alan boşalmalı; panodaki Türkçe metin aynı olmalıdır.
   Ctrl+V ile geri yapıştırın. Başka bir yerel düz metin düzenleyicisinde de
   yapıştırıp kopyalayarak uygulamaya dönüşü sınayın; yalnız test metni kullanın.
5. Araç izin veriyorsa çok satırlı örneği yapıştırın. Satır sırası ve boşluklar
   korunmalı; varsa CRLF/LF dönüşümünü kaydedin. Tek satırlı form alanına çok
   satırlı davranış şartı uygulamayın.
6. Düzenlemeyi araç tablosundaki yöntemle bitirin. Uygulamanın Geri al ve Yinele
   komutlarını çalıştırın: geri alma önceki içeriği, yineleme doğru Türkçe
   içeriği getirmelidir. Başka belge veya alan değişmemelidir.
7. Türkçe klasör/dosya adıyla kaydedin. Belgeyi kapatıp diskten yeniden açın.
   Metin, görünüm ve ilgili alan/açıklama değeri aynı olmalıdır. Belgeyi bağımsız
   PDF okuyucuda da açıp görünümü kontrol edin; henüz yeniden kaydetmeyin.
8. PdfCraft'ta doğru düzenleyiciyi yeniden açın, sona ` Şubat` ekleyin,
   kaydedin ve tekrar açın. Yeni değer ile önceki bütün harfler korunmalıdır.
   Yeniden açılmış düzenleyicide Ctrl+A/C ile kopya karşılaştırmasını da tekrarlayın.

Ctrl+Z ile Geri al'ı, Ctrl+Y veya uygulamanın Yinele komutunu ayrıca deneyin.
Tuşun başka bir kontrole gitmesi, çalışmayan kısayol veya yalnız fareyle
başarılı olma durumu bulguya yazılır; fiziksel kısayol adımı PASS sayılmaz.

### Araç ve sonuç tablosu

Her Q/F hücresi, yukarıdaki sekiz adımın o araç için tamamını kapsar. Bir alt
adım çalışmadıysa ilgili kimliği ve başarısız adımı bulguya yazın.
Pano önkoşulu sağlanmadığında 3–5 ve 8'deki kopyalama uygulanmaz; tüm-akış
hücresi NOT VERIFIED kalır. Sadece fiziksel yazma gözlemi ayrı kaydedilebilir.

| Kimlik | Araç / başlangıç ve bitirme yöntemi | Windows Q ile fiziksel giriş | Windows F ile fiziksel giriş | Bulgu kimliği |
|---|---|---|---|---|
| G-01 | Yeni sayfa metni: Metin ekle, boş sayfaya tıkla; Bitti ile tamamla | NOT VERIFIED | NOT VERIFIED | |
| G-02 | Mevcut paragraf: basit sentetik paragrafı Metin düzenle ile aç, seçip değiştir; düzenlemeyi uygula | NOT VERIFIED | NOT VERIFIED | |
| G-03 | Form: geniş, düzenlenebilir tek satırlı metin alanına tıkla; Enter veya başka alana geçerek tamamla | NOT VERIFIED | NOT VERIFIED | |
| G-04 | Metin kutusu: açıklama/metin kutusu aracında düzenleyiciyi aç; Gönder/Bitti ile tamamla | NOT VERIFIED | NOT VERIFIED | |
| G-05 | Fill & Sign metni: Doldur ve İmzala'nın metin aracını aç; metin kutusunun dışına tıklayarak tamamla | NOT VERIFIED | NOT VERIFIED | |

G-05, görsel metin ekleme akışıdır; sertifikalı dijital imza kabulü değildir.

### Pano karşılaştırması

Bu sürüm için pano içeriği okuma/yazma komutu verilmez. İncelenmiş bir kayıpsız
sağlayıcı eklendiğinde yalnız sentetik kopyanın birebir Ordinal karşılaştırması
yapılmalıdır. ` Şubat` son eki beklenen değere de eklenir. Boşlukları silerek,
harfleri küçülterek veya Unicode'u normalleştirerek farkı PASS'e çevirmeyin.
Çok satırlı kanal dönüşümü ayrıca kaydedilir. Beklenmeyen fark yalnız sentetik
örneğin kod noktaları ve yeniden üretim adımlarıyla raporlanır.

## NFD sınırı ve belgeyi koruma

Yukarıdaki fiziksel referanslar önceden birleşmiş karakterlerdir (NFC).
Desteklenen Latin NFD dizileri sayfa metni, form, metin kutusu, açıklama balonu,
Fill & Sign metni, üst/alt bilgi, filigran ve görsel imza/paraf akışlarında korunur.
Görünür ve görünmez OCR de desteklenen dizilerin özgün kod noktalarını saklar.

İmza PDF'i Stamp/PCOutline vektör görünümüdür; searchable/ToUnicode imza
iddiası yoktur. Mantıksal özgün metnin kanıtı editör ve izole tercihlerdir.
Arama eşdeğerliği, EventText/Paste/IME testleri ve eski native kabul; fiziksel
NFD Q/F veya NFD OS panosu sonucunun yerine geçmez.

NFD örneği denenirse ayrı bulgu kaydı açın. Desteklenmeyen girdi açıkça
reddediliyorsa önceki belge ve kullanıcının taslağı korunmalıdır. Değer saklanan
bir yolda özgün karakterler sessizce ASCII'ye, `?` işaretine veya başka Unicode
biçimine dönüştürülmemelidir. Mevcut görünüm uyarısı ve düzleştirme engeli,
şekillendirme düzeltilene kadar korunur. Uyarılı/yanlış çizilen çıktı için NFD
yazımı PASS verilmez; özgün kabul dosyasını düzleştirerek veya üstüne yazarak
bulguyu gizlemeyin.

## Bulgu ve kabul kararı

Her bulgu için şu kaydı çoğaltın:

| Alan | Değer |
|---|---|
| Bulgu kimliği / araç kimliği / adım | |
| Windows girdi düzeni / fiziksel klavye | |
| Sonuç | PASS / FAIL / NOT VERIFIED: |
| Beklenen / gözlenen davranış | |
| Sentetik metin ve gerekiyorsa kod noktaları | |
| Yeniden üretim adımları | |
| Kaydet/yeniden aç ve bağımsız okuyucu sonucu | |
| Özgün dosya korunuyor mu? | |
| Kanıt dosyaları / SHA-256 | |

**PASS:** satırın bütün adımları, bu kayıttaki binary ve gerçek tuş basışlarıyla
geçti. **FAIL:** denenmiş bir adım beklenen sonucu vermedi. **NOT VERIFIED:**
çalıştırılmadı, düzen/donanım yoktu veya yeterli kanıt alınmadı.

Q sonucunu F'ye, bir aracın sonucunu diğer araca veya eski binary sonucunu yeni
binary'ye taşımayın. Fiziksel satırlar tamamlanmadan TR-08'in fiziksel kabulünü
kapatmayın. Bu liste tamamlansa da kalan NFD kapsamı ve diğer P0 kabul koşulları
kapanmadan Aşama 1 ya da Windows 1.0 tamamlandı denmez.

**GYK-1: ASKIDA — geliştirme sürüyor.** Bu belge yeni yayın/merge/deploy yetkisi
vermez; temiz Windows ve fiziksel yazdırma da NOT VERIFIED kalır.
