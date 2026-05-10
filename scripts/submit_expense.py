#!/usr/bin/env python3
"""Parse a receipt PDF and fill the HMC INQ expense reimbursement form."""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


DEFAULT_PORTAL_URL = "https://class.hmcinq.com/expenses"
DEFAULT_PROFILE_DIR = Path.home() / ".expense-portal-browser"


@dataclass(frozen=True)
class ExpenseDetails:
    amount: str
    vendor: str
    description: str


def extract_pdf_text(path: Path) -> str:
    pdftotext = shutil.which("pdftotext")
    if pdftotext:
        result = subprocess.run(
            [pdftotext, "-layout", str(path), "-"],
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        return result.stdout

    for module_name in ("pypdf", "PyPDF2"):
        try:
            module = __import__(module_name)
        except ImportError:
            continue

        reader_cls = getattr(module, "PdfReader")
        reader = reader_cls(str(path))
        return "\n".join(page.extract_text() or "" for page in reader.pages)

    raise RuntimeError("Install pdftotext, pypdf, or PyPDF2 to parse PDF receipts.")


def money_to_decimal(value: str) -> str:
    return f"{float(value.replace(',', '')):.2f}"


def extract_amount(text: str) -> str:
    patterns = (
        r"Amount\s+Due\s+\$([0-9][0-9,]*(?:\.[0-9]{2})?)",
        r"Total\s+Due\s+\$([0-9][0-9,]*(?:\.[0-9]{2})?)",
        r"Balance\s+Due\s+\$([0-9][0-9,]*(?:\.[0-9]{2})?)",
        r"Grand\s+Total\s+\$([0-9][0-9,]*(?:\.[0-9]{2})?)",
        r"\bTotal\b\s+\$([0-9][0-9,]*(?:\.[0-9]{2})?)",
    )
    for pattern in patterns:
        matches = re.findall(pattern, text, flags=re.IGNORECASE)
        if matches:
            return money_to_decimal(matches[-1])
    raise ValueError("Could not find a total amount in the receipt text.")


def extract_vendor(text: str, receipt_path: Path) -> str:
    title_match = re.search(r"\|\s*[^|\n]+[–-]\s*([A-Za-z][A-Za-z0-9 .,&]+)", text)
    if title_match:
        return title_match.group(1).strip()

    filename = receipt_path.stem
    filename_vendor = re.search(r"[–-]\s*([A-Za-z][A-Za-z0-9 .,&]+)$", filename)
    if filename_vendor:
        return filename_vendor.group(1).strip()

    for candidate in ("Vercel", "OpenAI", "Anthropic", "Supabase", "GitHub", "Google"):
        if re.search(rf"\b{re.escape(candidate)}\b", text, flags=re.IGNORECASE):
            return candidate

    first_words = re.search(r"^\s*([A-Za-z][A-Za-z0-9 .,&]{2,40})", text, flags=re.MULTILINE)
    if first_words:
        return first_words.group(1).strip()
    raise ValueError("Could not infer a vendor from the receipt text.")


def extract_description(text: str, vendor: str) -> str:
    if vendor.lower() == "vercel":
        plan = re.search(r"([A-Z][a-z]+\s+\d{4}:\s*Monthly\s+Pro\s+Plan)", text)
        products: list[str] = []
        if re.search(r"\bPro\s+1\s+\$[0-9]", text):
            products.append("Vercel platform Pro")
        if re.search(r"Additional\s+Team\s+Seats?", text, flags=re.IGNORECASE):
            products.append("Additional Team Seat")

        prefix = plan.group(1).replace(":", "") if plan else "Vercel subscription"
        if products:
            return f"{prefix} - {' + '.join(products)}"
        return prefix

    invoice_line = re.search(r"([A-Z][a-z]+\s+\d{4}:?[^\n]{0,80})", text)
    if invoice_line:
        return re.sub(r"\s+", " ", invoice_line.group(1)).strip()

    return f"{vendor} expense"


def parse_receipt(path: Path, amount: str | None, vendor: str | None, description: str | None) -> ExpenseDetails:
    text = extract_pdf_text(path)
    parsed_vendor = vendor or extract_vendor(text, path)
    return ExpenseDetails(
        amount=money_to_decimal(amount) if amount else extract_amount(text),
        vendor=parsed_vendor,
        description=description or extract_description(text, parsed_vendor),
    )


def fill_portal(
    *,
    receipt_path: Path,
    details: ExpenseDetails,
    portal_url: str,
    profile_dir: Path,
    submit: bool,
) -> None:
    try:
        from playwright.sync_api import TimeoutError as PlaywrightTimeoutError
        from playwright.sync_api import sync_playwright
    except ImportError as exc:
        raise RuntimeError("Python Playwright is required for portal automation.") from exc

    with sync_playwright() as playwright:
        context = playwright.chromium.launch_persistent_context(
            str(profile_dir),
            headless=False,
            accept_downloads=True,
        )
        page = context.pages[0] if context.pages else context.new_page()
        page.goto(portal_url, wait_until="domcontentloaded")

        try:
            page.locator("#expenseAmount").wait_for(timeout=5000)
        except PlaywrightTimeoutError:
            print("Log in if prompted. Waiting for the expense form to appear...", file=sys.stderr)
            page.locator("#expenseAmount").wait_for(timeout=300000)

        page.locator("#expenseAmount").fill(details.amount)
        page.locator("#expenseRecipient").fill(details.vendor)
        page.locator("#expenseDescription").fill(details.description)
        page.locator("#expenseReceipt").set_input_files(str(receipt_path))

        if submit:
            page.locator("#expenseForm button[type='submit']").click()
            page.wait_for_timeout(1500)
            print("Submitted expense form.")
        else:
            print("Filled the expense form and attached the receipt. Re-run with --submit to submit.")

        context.close()


def existing_pdf(path_text: str) -> Path:
    path = Path(os.path.expanduser(path_text)).resolve()
    if not path.exists():
        raise argparse.ArgumentTypeError(f"Receipt not found: {path}")
    if path.suffix.lower() != ".pdf":
        raise argparse.ArgumentTypeError("This automation currently expects a PDF receipt.")
    return path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("receipt", type=existing_pdf, help="Path to the receipt PDF.")
    parser.add_argument("--portal-url", default=DEFAULT_PORTAL_URL)
    parser.add_argument("--profile-dir", type=Path, default=DEFAULT_PROFILE_DIR)
    parser.add_argument("--amount", help="Override the parsed amount.")
    parser.add_argument("--vendor", help="Override the parsed vendor.")
    parser.add_argument("--description", help="Override the parsed description.")
    parser.add_argument("--fill", action="store_true", help="Fill the portal form and attach the receipt.")
    parser.add_argument("--submit", action="store_true", help="Submit the filled form.")
    args = parser.parse_args()

    details = parse_receipt(args.receipt, args.amount, args.vendor, args.description)
    print(f"Amount: {details.amount}")
    print(f"Paid To: {details.vendor}")
    print(f"Description: {details.description}")
    print(f"Receipt: {args.receipt}")

    if args.fill or args.submit:
        fill_portal(
            receipt_path=args.receipt,
            details=details,
            portal_url=args.portal_url,
            profile_dir=args.profile_dir.expanduser().resolve(),
            submit=args.submit,
        )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
