# Azure: one VM with k3s (a single-node Kubernetes) running BMS.

# Everything of this project in Azure lives in one resource group:
# deleting the group deletes all of it.
resource "azurerm_resource_group" "bms" {
  name     = "bms"
  location = var.azure_location
}

resource "azurerm_virtual_network" "bms" {
  name                = "bms-vnet"
  location            = azurerm_resource_group.bms.location
  resource_group_name = azurerm_resource_group.bms.name
  address_space       = ["10.10.0.0/16"]
}

resource "azurerm_subnet" "bms" {
  name                 = "bms-subnet"
  resource_group_name  = azurerm_resource_group.bms.name
  virtual_network_name = azurerm_virtual_network.bms.name
  address_prefixes     = ["10.10.1.0/24"]
}

# Part of the DNS name: names under cloudapp.azure.com are global.
resource "random_string" "dns_suffix" {
  length  = 6
  special = false
  upper   = false
}

# Static public address with a free DNS name:
#   bms-<suffix>.denmarkeast.cloudapp.azure.com
# The name is what the HTTPS certificate is issued for.
resource "azurerm_public_ip" "bms" {
  name                = "bms-ip"
  location            = azurerm_resource_group.bms.location
  resource_group_name = azurerm_resource_group.bms.name
  allocation_method   = "Static"
  sku                 = "Standard"
  domain_name_label   = "bms-${random_string.dns_suffix.result}"
}

# The firewall (network security group). Lower priority number = checked first;
# anything not allowed is denied by Azure's default rules.
resource "azurerm_network_security_group" "bms" {
  name                = "bms-nsg"
  location            = azurerm_resource_group.bms.location
  resource_group_name = azurerm_resource_group.bms.name

  security_rule {
    name                       = "ssh-admin"
    priority                   = 100
    direction                  = "Inbound"
    access                     = "Allow"
    protocol                   = "Tcp"
    source_address_prefix      = local.admin_cidr
    source_port_range          = "*"
    destination_address_prefix = "*"
    destination_port_range     = "22"
  }

  # kubectl from the admin's machine.
  security_rule {
    name                       = "kubernetes-api-admin"
    priority                   = 110
    direction                  = "Inbound"
    access                     = "Allow"
    protocol                   = "Tcp"
    source_address_prefix      = local.admin_cidr
    source_port_range          = "*"
    destination_address_prefix = "*"
    destination_port_range     = "6443"
  }

  # The BMS API for everyone, over HTTPS. Port 80 is needed by Let's Encrypt to
  # verify the domain, and redirects everything else to HTTPS.
  security_rule {
    name                       = "web"
    priority                   = 120
    direction                  = "Inbound"
    access                     = "Allow"
    protocol                   = "Tcp"
    source_address_prefix      = "*"
    source_port_range          = "*"
    destination_address_prefix = "*"
    destination_port_ranges    = ["80", "443"]
  }
}

resource "azurerm_network_interface" "bms" {
  name                = "bms-nic"
  location            = azurerm_resource_group.bms.location
  resource_group_name = azurerm_resource_group.bms.name

  ip_configuration {
    name                          = "primary"
    subnet_id                     = azurerm_subnet.bms.id
    private_ip_address_allocation = "Dynamic"
    public_ip_address_id          = azurerm_public_ip.bms.id
  }
}

resource "azurerm_network_interface_security_group_association" "bms" {
  network_interface_id      = azurerm_network_interface.bms.id
  network_security_group_id = azurerm_network_security_group.bms.id
}

resource "azurerm_linux_virtual_machine" "bms" {
  name                  = "bms"
  location              = azurerm_resource_group.bms.location
  resource_group_name   = azurerm_resource_group.bms.name
  size                  = var.azure_vm_size
  admin_username        = "azureuser"
  network_interface_ids = [azurerm_network_interface.bms.id]

  # Key-only login; passwords are disabled.
  admin_ssh_key {
    username   = "azureuser"
    public_key = local.ssh_public_key
  }

  os_disk {
    caching              = "ReadWrite"
    storage_account_type = "StandardSSD_LRS"
    disk_size_gb         = 30
  }

  source_image_reference {
    publisher = "Canonical"
    offer     = "ubuntu-26_04-lts"
    sku       = "server"
    version   = "latest"
  }

  # cloud-init: installs k3s on the first boot.
  custom_data = base64encode(templatefile("${path.module}/cloud-init/k3s.yaml.tftpl", {
    fqdn      = azurerm_public_ip.bms.fqdn
    public_ip = local.azure_ip
  }))
}
