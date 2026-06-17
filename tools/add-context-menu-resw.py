#!/usr/bin/env python3
# Adds Shell_* localization keys for the 4 new context-menu verbs.
# (ExtractSmart, ExtractToSubfolder, ExtractDialog, CompressIndividually)
# Inserts before </root>. Idempotent.

import os
import re

ROOT = os.path.normpath(os.path.join(
    os.path.dirname(__file__), "..", "app", "OtterZip.App", "Strings"
))

LOCALES = {
    "en-US": {
        "ExtractSmart_Title": "Smart extract (&Z)",
        "ExtractSmart_Tooltip": "Auto-detect single-root layout and extract accordingly.",
        "ExtractToSubfolder_TitleFormat": "Extract to {0}\\ (&E)",
        "ExtractToSubfolder_Tooltip": "Extract into a new folder named after the archive.",
        "ExtractDialog_Title": "Extract with OtterZip...(&B)",
        "ExtractDialog_Tooltip": "Open the OtterZip extract dialog (destination, overwrite, password).",
        "CompressIndividually_Title": "Compress each item (&U)",
        "CompressIndividually_Tooltip": "Create one archive per selected item, using your default format.",
    },
    "ko-KR": {
        "ExtractSmart_Title": "알아서 풀기(&Z)",
        "ExtractSmart_Tooltip": "압축 안에 루트 폴더가 하나면 그대로, 아니면 새 폴더에 풀기.",
        "ExtractToSubfolder_TitleFormat": "{0}\\에 풀기(&E)",
        "ExtractToSubfolder_Tooltip": "압축파일 이름의 새 폴더를 만들어 풀기.",
        "ExtractDialog_Title": "OtterZip으로 풀기...(&B)",
        "ExtractDialog_Tooltip": "대상 폴더/덮어쓰기/암호 옵션 다이얼로그 열기.",
        "CompressIndividually_Title": "각 항목별 압축(&U)",
        "CompressIndividually_Tooltip": "선택한 항목마다 압축 파일 1개씩 만들기 (기본 포맷 사용).",
    },
    "ja-JP": {
        "ExtractSmart_Title": "おまかせ展開(&Z)",
        "ExtractSmart_Tooltip": "ルートフォルダーが1つなら直接、複数なら新しいフォルダーへ展開。",
        "ExtractToSubfolder_TitleFormat": "{0}\\に展開(&E)",
        "ExtractToSubfolder_Tooltip": "アーカイブ名のフォルダーを作成して展開。",
        "ExtractDialog_Title": "OtterZipで展開...(&B)",
        "ExtractDialog_Tooltip": "展開先・上書き・パスワードのオプションダイアログを開く。",
        "CompressIndividually_Title": "個別に圧縮(&U)",
        "CompressIndividually_Tooltip": "選択した項目ごとにアーカイブを作成 (既定の形式)。",
    },
    "zh-CN": {
        "ExtractSmart_Title": "智能解压(&Z)",
        "ExtractSmart_Tooltip": "若压缩包内为单一根目录则原地解压, 否则新建文件夹。",
        "ExtractToSubfolder_TitleFormat": "解压到 {0}\\(&E)",
        "ExtractToSubfolder_Tooltip": "新建以压缩包命名的文件夹再解压。",
        "ExtractDialog_Title": "用 OtterZip 解压...(&B)",
        "ExtractDialog_Tooltip": "打开解压选项对话框 (目标/覆盖/密码)。",
        "CompressIndividually_Title": "逐项压缩(&U)",
        "CompressIndividually_Tooltip": "为每个所选项创建独立的压缩文件 (使用默认格式)。",
    },
    "de-DE": {
        "ExtractSmart_Title": "Intelligent entpacken (&Z)",
        "ExtractSmart_Tooltip": "Bei einzelnem Stammordner direkt entpacken, sonst neuen Ordner anlegen.",
        "ExtractToSubfolder_TitleFormat": "In {0}\\ entpacken (&E)",
        "ExtractToSubfolder_Tooltip": "In einen neuen Ordner mit dem Archivnamen entpacken.",
        "ExtractDialog_Title": "Mit OtterZip entpacken...(&B)",
        "ExtractDialog_Tooltip": "OtterZip-Entpacken-Dialog öffnen (Ziel, Überschreiben, Passwort).",
        "CompressIndividually_Title": "Einzeln komprimieren (&U)",
        "CompressIndividually_Tooltip": "Pro ausgewähltem Element ein Archiv erstellen (Standardformat).",
    },
    "fr-FR": {
        "ExtractSmart_Title": "Extraction intelligente (&Z)",
        "ExtractSmart_Tooltip": "Extraction directe si un seul dossier racine, sinon nouveau dossier.",
        "ExtractToSubfolder_TitleFormat": "Extraire dans {0}\\ (&E)",
        "ExtractToSubfolder_Tooltip": "Extraire dans un nouveau dossier portant le nom de l'archive.",
        "ExtractDialog_Title": "Extraire avec OtterZip...(&B)",
        "ExtractDialog_Tooltip": "Ouvrir le dialogue d'extraction (destination, écraser, mot de passe).",
        "CompressIndividually_Title": "Compresser chaque élément (&U)",
        "CompressIndividually_Tooltip": "Créer une archive par élément (format par défaut).",
    },
    "es-ES": {
        "ExtractSmart_Title": "Extraer inteligentemente (&Z)",
        "ExtractSmart_Tooltip": "Extraer directo si hay un solo directorio raíz, si no crear carpeta nueva.",
        "ExtractToSubfolder_TitleFormat": "Extraer a {0}\\ (&E)",
        "ExtractToSubfolder_Tooltip": "Extraer en una carpeta nueva con el nombre del archivo.",
        "ExtractDialog_Title": "Extraer con OtterZip...(&B)",
        "ExtractDialog_Tooltip": "Abrir diálogo de extracción (destino, sobrescribir, contraseña).",
        "CompressIndividually_Title": "Comprimir cada elemento (&U)",
        "CompressIndividually_Tooltip": "Crear un archivo por elemento seleccionado (formato predeterminado).",
    },
    "pt-BR": {
        "ExtractSmart_Title": "Extração inteligente (&Z)",
        "ExtractSmart_Tooltip": "Extrair direto se houver uma única pasta raiz, senão criar nova pasta.",
        "ExtractToSubfolder_TitleFormat": "Extrair em {0}\\ (&E)",
        "ExtractToSubfolder_Tooltip": "Extrair em uma nova pasta com o nome do arquivo.",
        "ExtractDialog_Title": "Extrair com OtterZip...(&B)",
        "ExtractDialog_Tooltip": "Abrir diálogo de extração (destino, sobrescrever, senha).",
        "CompressIndividually_Title": "Comprimir cada item (&U)",
        "CompressIndividually_Tooltip": "Criar um arquivo por item selecionado (formato padrão).",
    },
    "ru-RU": {
        "ExtractSmart_Title": "Умное извлечение (&Z)",
        "ExtractSmart_Tooltip": "Прямое извлечение при одной корневой папке, иначе создать новую папку.",
        "ExtractToSubfolder_TitleFormat": "Извлечь в {0}\\ (&E)",
        "ExtractToSubfolder_Tooltip": "Извлечь в новую папку с именем архива.",
        "ExtractDialog_Title": "Извлечь с OtterZip...(&B)",
        "ExtractDialog_Tooltip": "Открыть диалог извлечения (назначение, перезапись, пароль).",
        "CompressIndividually_Title": "Сжать каждый элемент (&U)",
        "CompressIndividually_Tooltip": "Создать архив для каждого выбранного элемента (формат по умолчанию).",
    },
    "it-IT": {
        "ExtractSmart_Title": "Estrazione intelligente (&Z)",
        "ExtractSmart_Tooltip": "Estrazione diretta se c'è una sola cartella radice, altrimenti nuova cartella.",
        "ExtractToSubfolder_TitleFormat": "Estrai in {0}\\ (&E)",
        "ExtractToSubfolder_Tooltip": "Estrai in una nuova cartella con il nome dell'archivio.",
        "ExtractDialog_Title": "Estrai con OtterZip...(&B)",
        "ExtractDialog_Tooltip": "Apri la finestra di estrazione (destinazione, sovrascrittura, password).",
        "CompressIndividually_Title": "Comprimi ogni elemento (&U)",
        "CompressIndividually_Tooltip": "Crea un archivio per ogni elemento selezionato (formato predefinito).",
    },
}

KEY_ORDER = [
    "ExtractSmart_Title", "ExtractSmart_Tooltip",
    "ExtractToSubfolder_TitleFormat", "ExtractToSubfolder_Tooltip",
    "ExtractDialog_Title", "ExtractDialog_Tooltip",
    "CompressIndividually_Title", "CompressIndividually_Tooltip",
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

    if 'name="Shell_ExtractSmart_Title"' in content:
        print(f"[skip] {locale} (already has new keys)")
        continue

    block_lines = ['  <!-- Bandizip 4-context parity verbs (2026-05-19) -->']
    for key in KEY_ORDER:
        val = xml_escape(strings[key])
        block_lines.append(f'  <data name="Shell_{key}"><value>{val}</value></data>')
    block = "\n".join(block_lines) + "\n"

    new_content = re.sub(r"\s*</root>\s*$", "\n" + block + "</root>\n", content)
    with open(path, "w", encoding="utf-8") as fp:
        fp.write("﻿")
        fp.write(new_content)
    print(f"[ok]   {locale}")
