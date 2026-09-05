# Microsoft Store Partner Center Submission Guide

Follow these steps to publish Marknexia to the official Microsoft Store.

---

## Step 1: Reserve Product Name
1. Sign in to [Microsoft Partner Center](https://partner.microsoft.com/dashboard/apps-and-games/overview).
2. Click **Create a new product** -> Select **MSIX or PWA app**.
3. Enter **Marknexia** as the app name and click **Reserve product name**.

---

## Step 2: Retrieve App Identity from Partner Center
1. Navigate to **Product management** -> **App identity**.
2. Copy the following values:
   * **Package/Identity/Name** (e.g., `45678YourCompany.Marknexia`)
   * **Package/Identity/Publisher** (e.g., `CN=XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`)
   * **Package/Properties/PublisherDisplayName** (e.g., `Your Company Name`)

---

## Step 3: Package Alignment (Optional Automated Tool)
If Partner Center generated a specific Publisher ID for your account, run the included script to align the package before upload:
```powershell
./scripts/prepare-store-package.ps1 -PackageName "<Your-Package-Name>" -PublisherId "<Your-Publisher-ID>" -PublisherDisplayName "<Your-Display-Name>"
```
This generates `store-submission/Marknexia-Store-Ready.msix` matching your exact Partner Center identity.

---

## Step 4: Create Submission in Partner Center
1. Under **Submission 1**, complete the sections:
   * **Pricing and availability**: Free, worldwide distribution.
   * **Properties**: Category: *Developer tools > Documentation & Utilities*.
   * **Age ratings**: Complete the IARC questionnaire (Rating: Everyone / All Ages).
   * **Packages**: Drag and drop `store-submission/Marknexia-v1.0.0-win-x64.msix` (or `Marknexia-Store-Ready.msix`).
   * **Store listings**: Paste the copy from `store-submission/STORE-LISTING.md` and upload the assets from `store-submission/assets/`.
2. Click **Submit to the Store**.

Microsoft will automatically sign the package with Microsoft Store trusted certificates upon certification, allowing all Windows users to install Marknexia with one click!
