#!/usr/bin/env python3
# Add Shell_* localization keys to all 10 .resw files (one-shot script).
# Inserts a block of <data> entries right before </root>. Idempotent —
# skips locales that already have any of the new keys.

import os
import re
import sys

ROOT = os.path.join(os.path.dirname(__file__), "..", "app", "OtterZip.App", "Strings")
ROOT = os.path.normpath(ROOT)

LOCALES = {
    "en-US": {
        "ExtractHere_Title": "Extract here (OtterZip)",
        "ExtractHere_Tooltip": "Extract this archive next to its current folder.",
        "ExtractHereSubmenu_Title": "Extract here",
        "CompressDialog_Title": "Compress with OtterZip... (&O)",
        "CompressDialog_Tooltip": "Create a new archive from the selected files.",
        "CompressQuick_TitleFormat": "Compress to {0}{1} (&{2})",
        "CompressZipQuick_Tooltip": "Compress selected items to a ZIP archive (no dialog).",
        "Compress7zQuick_Tooltip": "Compress selected items to a 7-Zip archive (no dialog).",
        "OtterzipMenu_Title": "OtterZip",
        "OtterzipMenu_Tooltip": "OtterZip — fast archive tool",
    },
    "ko-KR": {
        "ExtractHere_Title": "OtterZip으로 풀기",
        "ExtractHere_Tooltip": "이 압축 파일을 현재 폴더에 풀기.",
        "ExtractHereSubmenu_Title": "여기에 풀기",
        "CompressDialog_Title": "OtterZip으로 압축...(&O)",
        "CompressDialog_Tooltip": "선택한 항목으로 새 압축 파일 만들기.",
        "CompressQuick_TitleFormat": "{0}{1}으로 압축(&{2})",
        "CompressZipQuick_Tooltip": "선택한 항목을 ZIP 압축 파일로 만들기 (대화상자 없음).",
        "Compress7zQuick_Tooltip": "선택한 항목을 7z 압축 파일로 만들기 (대화상자 없음).",
        "OtterzipMenu_Title": "OtterZip",
        "OtterzipMenu_Tooltip": "OtterZip — 빠른 압축 도구",
    },
    "ja-JP": {
        "ExtractHere_Title": "OtterZipで展開",
        "ExtractHere_Tooltip": "このアーカイブを現在のフォルダーに展開します。",
        "ExtractHereSubmenu_Title": "ここに展開",
        "CompressDialog_Title": "OtterZipで圧縮...(&O)",
        "CompressDialog_Tooltip": "選択した項目から新しいアーカイブを作成します。",
        "CompressQuick_TitleFormat": "{0}{1}に圧縮(&{2})",
        "CompressZipQuick_Tooltip": "選択した項目をZIPアーカイブに圧縮します（ダイアログなし）。",
        "Compress7zQuick_Tooltip": "選択した項目を7zアーカイブに圧縮します(ダイアログなし)。",
        "OtterzipMenu_Title": "OtterZip",
        "OtterzipMenu_Tooltip": "OtterZip — 高速アーカイブツール",
    },
    "zh-CN": {
        "ExtractHere_Title": "使用 OtterZip 解压到此处",
        "ExtractHere_Tooltip": "将此压缩文件解压到当前文件夹。",
        "ExtractHereSubmenu_Title": "解压到当前位置",
        "CompressDialog_Title": "用 OtterZip 压缩...(&O)",
        "CompressDialog_Tooltip": "用所选项目创建新的压缩文件。",
        "CompressQuick_TitleFormat": "压缩为 {0}{1}(&{2})",
        "CompressZipQuick_Tooltip": "将所选项目压缩为 ZIP 文件(无对话框)。",
        "Compress7zQuick_Tooltip": "将所选项目压缩为 7z 文件(无对话框)。",
        "OtterzipMenu_Title": "OtterZip",
        "OtterzipMenu_Tooltip": "OtterZip — 快速压缩工具",
    },
    "de-DE": {
        "ExtractHere_Title": "Mit OtterZip hier entpacken",
        "ExtractHere_Tooltip": "Dieses Archiv im aktuellen Ordner entpacken.",
        "ExtractHereSubmenu_Title": "Hier entpacken",
        "CompressDialog_Title": "Mit OtterZip komprimieren...(&O)",
        "CompressDialog_Tooltip": "Aus den ausgewählten Dateien ein neues Archiv erstellen.",
        "CompressQuick_TitleFormat": "Zu {0}{1} komprimieren (&{2})",
        "CompressZipQuick_Tooltip": "Ausgewählte Elemente ohne Dialog zu ZIP-Archiv komprimieren.",
        "Compress7zQuick_Tooltip": "Ausgewählte Elemente ohne Dialog zu 7-Zip-Archiv komprimieren.",
        "OtterzipMenu_Title": "OtterZip",
        "OtterzipMenu_Tooltip": "OtterZip — schnelles Archivierungstool",
    },
    "fr-FR": {
        "ExtractHere_Title": "Extraire ici (OtterZip)",
        "ExtractHere_Tooltip": "Extraire cette archive dans le dossier actuel.",
        "ExtractHereSubmenu_Title": "Extraire ici",
        "CompressDialog_Title": "Compresser avec OtterZip...(&O)",
        "CompressDialog_Tooltip": "Créer une nouvelle archive à partir des fichiers sélectionnés.",
        "CompressQuick_TitleFormat": "Compresser en {0}{1} (&{2})",
        "CompressZipQuick_Tooltip": "Compresser les éléments en archive ZIP (sans boîte de dialogue).",
        "Compress7zQuick_Tooltip": "Compresser les éléments en archive 7-Zip (sans boîte de dialogue).",
        "OtterzipMenu_Title": "OtterZip",
        "OtterzipMenu_Tooltip": "OtterZip — outil d'archive rapide",
    },
    "es-ES": {
        "ExtractHere_Title": "Extraer aquí (OtterZip)",
        "ExtractHere_Tooltip": "Extraer este archivo en la carpeta actual.",
        "ExtractHereSubmenu_Title": "Extraer aquí",
        "CompressDialog_Title": "Comprimir con OtterZip...(&O)",
        "CompressDialog_Tooltip": "Crear un nuevo archivo a partir de los elementos seleccionados.",
        "CompressQuick_TitleFormat": "Comprimir a {0}{1} (&{2})",
        "CompressZipQuick_Tooltip": "Comprimir los elementos seleccionados a archivo ZIP (sin diálogo).",
        "Compress7zQuick_Tooltip": "Comprimir los elementos seleccionados a archivo 7-Zip (sin diálogo).",
        "OtterzipMenu_Title": "OtterZip",
        "OtterzipMenu_Tooltip": "OtterZip — herramienta de archivo rápida",
    },
    "pt-BR": {
        "ExtractHere_Title": "Extrair aqui (OtterZip)",
        "ExtractHere_Tooltip": "Extrair este arquivo na pasta atual.",
        "ExtractHereSubmenu_Title": "Extrair aqui",
        "CompressDialog_Title": "Comprimir com OtterZip...(&O)",
        "CompressDialog_Tooltip": "Criar um novo arquivo a partir dos itens selecionados.",
        "CompressQuick_TitleFormat": "Comprimir para {0}{1} (&{2})",
        "CompressZipQuick_Tooltip": "Comprimir itens selecionados em arquivo ZIP (sem diálogo).",
        "Compress7zQuick_Tooltip": "Comprimir itens selecionados em arquivo 7-Zip (sem diálogo).",
        "OtterzipMenu_Title": "OtterZip",
        "OtterzipMenu_Tooltip": "OtterZip — ferramenta de arquivo rápida",
    },
    "ru-RU": {
        "ExtractHere_Title": "Извлечь сюда (OtterZip)",
        "ExtractHere_Tooltip": "Извлечь архив в текущую папку.",
        "ExtractHereSubmenu_Title": "Извлечь сюда",
        "CompressDialog_Title": "Сжать с помощью OtterZip...(&O)",
        "CompressDialog_Tooltip": "Создать новый архив из выбранных файлов.",
        "CompressQuick_TitleFormat": "Сжать в {0}{1} (&{2})",
        "CompressZipQuick_Tooltip": "Сжать выбранные элементы в ZIP-архив (без диалога).",
        "Compress7zQuick_Tooltip": "Сжать выбранные элементы в 7-Zip-архив (без диалога).",
        "OtterzipMenu_Title": "OtterZip",
        "OtterzipMenu_Tooltip": "OtterZip — быстрый архиватор",
    },
    "it-IT": {
        "ExtractHere_Title": "Estrai qui (OtterZip)",
        "ExtractHere_Tooltip": "Estrai questo archivio nella cartella corrente.",
        "ExtractHereSubmenu_Title": "Estrai qui",
        "CompressDialog_Title": "Comprimi con OtterZip...(&O)",
        "CompressDialog_Tooltip": "Crea un nuovo archivio dagli elementi selezionati.",
        "CompressQuick_TitleFormat": "Comprimi in {0}{1} (&{2})",
        "CompressZipQuick_Tooltip": "Comprimi gli elementi selezionati in un archivio ZIP (senza dialogo).",
        "Compress7zQuick_Tooltip": "Comprimi gli elementi selezionati in un archivio 7-Zip (senza dialogo).",
        "OtterzipMenu_Title": "OtterZip",
        "OtterzipMenu_Tooltip": "OtterZip — strumento di archiviazione veloce",
    },
}

KEY_ORDER = [
    "ExtractHere_Title", "ExtractHere_Tooltip", "ExtractHereSubmenu_Title",
    "CompressDialog_Title", "CompressDialog_Tooltip", "CompressQuick_TitleFormat",
    "CompressZipQuick_Tooltip", "Compress7zQuick_Tooltip",
    "OtterzipMenu_Title", "OtterzipMenu_Tooltip",
]

def xml_escape(s: str) -> str:
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")

for locale, strings in LOCALES.items():
    path = os.path.join(ROOT, locale, "Resources.resw")
    if not os.path.exists(path):
        print(f"[skip] {locale} (file missing)")
        continue

    with open(path, "r", encoding="utf-8-sig") as fp:
        content = fp.read()

    if 'name="Shell_ExtractHere_Title"' in content:
        print(f"[skip] {locale} (already has Shell_* keys)")
        continue

    block_lines = ['  <!-- Shell extension IExplorerCommand titles + tooltips (2026-05-18) -->']
    for key in KEY_ORDER:
        val = xml_escape(strings[key])
        block_lines.append(f'  <data name="Shell_{key}"><value>{val}</value></data>')
    block = "\n".join(block_lines) + "\n"

    new_content = re.sub(r"\s*</root>\s*$", "\n" + block + "</root>\n", content)
    with open(path, "w", encoding="utf-8") as fp:
        fp.write("﻿")
        fp.write(new_content)
    print(f"[ok]   {locale}")
