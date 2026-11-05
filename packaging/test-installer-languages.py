"""Source/catalog parity. Compiled MSI and native PKG checks run separately."""
import json
import pathlib
import re
import xml.etree.ElementTree as ET

root = pathlib.Path(__file__).resolve().parent
languages = json.loads((root / 'installer-languages.json').read_text(encoding='utf-8'))
assert set(languages) == {'en', 'de', 'fr', 'es', 'nl', 'it', 'hu', 'zh-Hans', 'zh-Hant',
                          'hi', 'ar', 'bn', 'pt', 'ru', 'id', 'ur', 'ja', 'ko', 'tr'}
assert list(languages)[0] == 'en', 'base MSI must be built before transforms'
assert {p.stem for p in (root / 'windows' / 'locales').glob('*.wxl')} == set(languages)
assert {p.stem for p in (root / 'macos' / 'resources').glob('*.lproj')} == set(languages)
catalogs = {}
for language, metadata in languages.items():
    xml = ET.parse(root / 'windows' / 'locales' / (language + '.wxl')).getroot()
    strings = {x.attrib['Id']: x.attrib['Value'] for x in xml}
    assert len(strings) == len(xml), (language, 'duplicate strings')
    assert xml.attrib['Culture'] == metadata['culture']
    assert int(xml.attrib['Codepage']) == metadata['codepage']
    assert int(strings['Language']) == metadata['language']
    assert int(strings['Codepage']) == metadata['codepage']
    if metadata['codepage'] == 65001:
        assert int(xml.attrib['SummaryInformationCodepage']) == 1252
        assert strings['PackageDescription'].isascii(), 'UTF-8 MSI summary must remain ANSI'
    assert bool(metadata.get('rtl')) == (language in {'ar', 'ur'})
    for value in strings.values():
        assert value.strip(), (language, 'empty translation')
        assert value.encode('cp' + str(metadata['codepage'])).decode('cp' + str(metadata['codepage'])) == value
    catalogs[language] = strings
reference = catalogs['en']
for language, strings in catalogs.items():
    assert strings.keys() == reference.keys(), language
    for key, value in strings.items():
        assert re.findall(r'\[[^]]+\]', value) == re.findall(r'\[[^]]+\]', reference[key]), (language, key, 'placeholders')

used = set()
for file in ['pdfcraft.wxs', 'installer-ui.wxs']:
    xml = ET.parse(root / 'windows' / file).getroot()
    for element in xml.iter():
        for name, value in element.attrib.items():
            used.update(re.findall(r'!\(loc\.([A-Za-z]+)\)', value))
            if name == 'Id':
                assert '!(loc.' not in value, (file, 'localized identifier')
        if element.tag.endswith('}Control') and 'Text' in element.attrib:
            assert '!(loc.' in element.attrib['Text'], element.attrib
        if element.tag.endswith('}Dialog'):
            assert '!(loc.' in element.attrib['Title'], element.attrib
            width, height = int(element.attrib['Width']), int(element.attrib['Height'])
            for control in element:
                if not control.tag.endswith('}Control'):
                    continue
                a = control.attrib
                assert int(a['X']) + int(a['Width']) <= width, a
                assert int(a['Y']) + int(a['Height']) <= height, a
assert used == set(reference), (used - set(reference), set(reference) - used)

mac_reference = None
for language in languages:
    source = (root / 'macos' / 'resources' / (language + '.lproj') / 'Localizable.strings').read_text(encoding='utf-8')
    table = {}
    for line in source.splitlines():
        if not line.strip() or line.startswith('//'):
            continue
        match = re.fullmatch(r'("(?:[^"\\]|\\.)*") = ("(?:[^"\\]|\\.)*");', line)
        assert match, (language, line)
        key, value = [json.loads(x) for x in match.groups()]
        assert key not in table and value.strip(), (language, key)
        table[key] = value
    if mac_reference is None:
        mac_reference = table.keys()
    assert table.keys() == mac_reference, language
print(f'ok: {len(languages)} complete catalogs, placeholders, codepages, native UI bounds, macOS strings')
