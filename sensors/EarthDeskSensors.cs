// EarthDeskSensors -- the hardware monitor that ships inside 地球桌面 (EarthDesk).
//
// A small Windows service built on LibreHardwareMonitorLib (MPL-2.0, shipped
// unmodified next to this file). It replaces running the LibreHardwareMonitor
// GUI in the background:
//
//   * no window, no tray icon -- nothing for the user to see or close;
//   * runs as LocalSystem from boot, so no UAC prompt and no logon task;
//   * the service manager restarts it if it ever dies (setup-sensors.ps1
//     configures the recovery actions), and it exits on its own if it hangs
//     or bloats, so that restart actually happens;
//   * no web server and no TCP port: EarthDesk reads it over a local named
//     pipe, so there is nothing to collide with (8085 is a popular port),
//     nothing for a proxy or VPN to swallow, and nothing on the network.
//
// Protocol: connect to \\.\pipe\EarthDeskSensors for reading; the service
// writes one UTF-8 JSON document and closes the pipe. Sensors are only read
// when someone asks (at most about once a second), so it costs nothing while
// EarthDesk is not running.
//
// Written in C# 5 on purpose: tools\build-sensors.ps1 compiles it with the
// csc.exe that ships inside every Windows (.NET Framework 4.x), so building
// EarthDesk needs no extra SDK.
//
//   EarthDeskSensors.exe            run as the service (started by the SCM)
//   EarthDeskSensors.exe --dump     print one reading and exit (run elevated)
//   EarthDeskSensors.exe --console  serve the pipe in a console window

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.IO.Pipes;
using System.Security.AccessControl;
using System.Security.Principal;
using System.ServiceProcess;
using System.Text;
using System.Threading;
using LibreHardwareMonitor.Hardware;

[assembly: System.Reflection.AssemblyTitle("EarthDesk hardware sensors")]
[assembly: System.Reflection.AssemblyProduct("EarthDesk")]
[assembly: System.Reflection.AssemblyVersion("1.1.3.0")]
[assembly: System.Reflection.AssemblyFileVersion("1.1.3.0")]

namespace EarthDesk.Sensors
{
    internal static class Program
    {
        public const string ServiceName = "EarthDeskSensors";
        public const string PipeName = "EarthDeskSensors";
        public const string Version = "1.1.3";

        private static int Main(string[] args)
        {
            bool dump = false, console = false;
            foreach (string a in args)
            {
                if (a == "--dump") dump = true;
                else if (a == "--console") console = true;
            }

            if (dump)
            {
                Monitor m = new Monitor();
                m.Open();
                m.Storage.Start(new ManualResetEvent(false));
                // The first CPU load reading is always 0; a second pass fixes it.
                m.Snapshot(true);
                for (int i = 0; i < 60 && m.Storage.Fragment.Length == 0; i++) Thread.Sleep(500);
                Console.Out.Write(m.Snapshot(true));
                m.Close();
                return 0;
            }
            if (console)
            {
                Server s = new Server();
                s.Start();
                Console.WriteLine("Serving \\\\.\\pipe\\" + PipeName + " -- press Enter to stop.");
                Console.ReadLine();
                s.Stop();
                return 0;
            }
            ServiceBase.Run(new Service());
            return 0;
        }
    }

    internal sealed class Service : ServiceBase
    {
        private Server _server;

        public Service()
        {
            ServiceName = Program.ServiceName;
            CanStop = true;
            CanShutdown = true;
            AutoLog = false;
        }

        protected override void OnStart(string[] args)
        {
            _server = new Server();
            _server.Start();
        }

        protected override void OnStop()
        {
            if (_server != null) _server.Stop();
        }

        protected override void OnShutdown()
        {
            OnStop();
        }
    }

    /// Append-only diagnostic log in %ProgramData%\EarthDesk\sensors.log,
    /// trimmed to its newer half past 256 KB. Readable by any user, which is
    /// how the app's settings page and a support request get to see it.
    internal static class Log
    {
        private static readonly object Gate = new object();

        public static string Folder
        {
            get
            {
                return Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData), "EarthDesk");
            }
        }

        public static void Write(string line)
        {
            try
            {
                lock (Gate)
                {
                    Directory.CreateDirectory(Folder);
                    string path = Path.Combine(Folder, "sensors.log");
                    FileInfo fi = new FileInfo(path);
                    if (fi.Exists && fi.Length > 256 * 1024)
                    {
                        string text = File.ReadAllText(path);
                        int cut = text.IndexOf('\n', text.Length / 2);
                        File.WriteAllText(path, cut >= 0 ? text.Substring(cut + 1) : "");
                    }
                    File.AppendAllText(path,
                        DateTime.Now.ToString("yyyy-MM-dd HH:mm:ss", CultureInfo.InvariantCulture) + " " + line + "\r\n",
                        Encoding.UTF8);
                }
            }
            catch
            {
                // Logging must never take the service down.
            }
        }
    }

    /// Writes LibreHardwareMonitor hardware as the JSON objects that go down
    /// the pipe. Shared by the core monitor and the storage worker.
    internal static class Emit
    {
        public static void Hardware(StringBuilder sb, IHardware h, IHardware parent, ref bool first, bool update, ref string error)
        {
            if (update)
            {
                try
                {
                    h.Update();
                }
                catch (Exception e)
                {
                    error = h.Name + ": " + e.Message;
                }
            }

            if (!first) sb.Append(',');
            first = false;
            sb.Append("{\"name\":").Append(Json.Str(h.Name));
            sb.Append(",\"type\":").Append(Json.Str(h.HardwareType.ToString()));
            sb.Append(",\"parent\":").Append(parent == null ? "null" : Json.Str(parent.Name));
            sb.Append(",\"sensors\":[");
            bool firstSensor = true;
            foreach (ISensor s in h.Sensors)
            {
                // History is for the LibreHardwareMonitor GUI's plots. Left on,
                // every sensor keeps a day of readings -- a slow leak in a
                // process that never restarts.
                if (s.ValuesTimeWindow != TimeSpan.Zero) s.ValuesTimeWindow = TimeSpan.Zero;
                float? v = s.Value;
                if (!v.HasValue || float.IsNaN(v.Value) || float.IsInfinity(v.Value)) continue;
                if (!firstSensor) sb.Append(',');
                firstSensor = false;
                sb.Append("{\"name\":").Append(Json.Str(s.Name));
                sb.Append(",\"type\":").Append(Json.Str(s.SensorType.ToString()));
                sb.Append(",\"value\":").Append(Json.Num(v.Value)).Append('}');
            }
            sb.Append("]}");

            foreach (IHardware sub in h.SubHardware)
            {
                Hardware(sb, sub, h, ref first, update, ref error);
            }
        }

        public static string Describe(Computer c)
        {
            List<string> parts = new List<string>();
            foreach (IHardware h in c.Hardware)
            {
                int n = h.Sensors.Length;
                foreach (IHardware sub in h.SubHardware) n += sub.Sensors.Length;
                parts.Add(h.HardwareType + " \"" + h.Name + "\" (" + n + ")");
            }
            return parts.Count == 0 ? "no hardware" : string.Join(", ", parts.ToArray());
        }
    }

    /// The fast part: CPU, graphics, memory, mainboard (Super I/O fans and
    /// temperatures), fan controllers. Read inline whenever EarthDesk asks.
    ///
    /// Disks are deliberately not in here. They are read over SATA / NVMe /
    /// USB-bridge commands, and one misbehaving drive (a USB enclosure that
    /// stops answering, a disk that times out every command) used to stall
    /// the whole reading -- 2 to 5 minutes just to open, and an update that
    /// hung long enough for the watchdog to restart the service, over and
    /// over. They live in StorageWorker, on their own thread.
    internal sealed class Monitor
    {
        private Computer _computer;
        private readonly object _gate = new object();
        private string _cached = "";
        private DateTime _cachedAt = DateTime.MinValue;
        private DateTime _openedAt = DateTime.MinValue;
        private string _lastError = "";
        private readonly StorageWorker _storage = new StorageWorker();

        /// Ticks at which the current hardware update began, 0 while idle. The
        /// watchdog reads it to notice an update that never returns.
        public long BusySince;

        public DateTime OpenedAt { get { return _openedAt; } }
        public StorageWorker Storage { get { return _storage; } }

        public bool IsOpen
        {
            get { lock (_gate) { return _computer != null; } }
        }

        public void Open()
        {
            Computer c = new Computer();
            c.IsCpuEnabled = true;
            c.IsGpuEnabled = true;
            c.IsMemoryEnabled = true;
            c.IsMotherboardEnabled = true;
            c.IsStorageEnabled = false;
            c.IsControllerEnabled = true;
            c.IsNetworkEnabled = false;
            c.IsPsuEnabled = false;
            c.IsBatteryEnabled = false;
            c.IsPowerMonitorEnabled = false;
            Stopwatch sw = Stopwatch.StartNew();
            c.Open();
            lock (_gate)
            {
                _computer = c;
                _openedAt = DateTime.UtcNow;
                _cached = "";
            }
            Log.Write("opened in " + sw.ElapsedMilliseconds + " ms: " + Emit.Describe(c));
        }

        public void Close()
        {
            lock (_gate)
            {
                if (_computer == null) return;
                try { _computer.Close(); }
                catch (Exception e) { Log.Write("close failed: " + e.Message); }
                _computer = null;
            }
        }

        /// Close and open again: a GPU driver that was not up yet at boot.
        public void Reopen(string why)
        {
            Log.Write("reopening: " + why);
            Close();
            Open();
        }

        public bool HasGpu()
        {
            lock (_gate)
            {
                if (_computer == null) return false;
                foreach (IHardware h in _computer.Hardware)
                {
                    if (h.HardwareType == HardwareType.GpuNvidia || h.HardwareType == HardwareType.GpuAmd ||
                        h.HardwareType == HardwareType.GpuIntel)
                        return true;
                }
                return false;
            }
        }

        /// The PawnIO driver's version as its installer registered it, or ""
        /// when it is not installed (the same test LibreHardwareMonitor uses).
        private static string PawnIoVersion()
        {
            const string key = @"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO";
            foreach (Microsoft.Win32.RegistryView view in new[] {
                Microsoft.Win32.RegistryView.Registry64, Microsoft.Win32.RegistryView.Registry32 })
            {
                try
                {
                    using (Microsoft.Win32.RegistryKey hive = Microsoft.Win32.RegistryKey.OpenBaseKey(
                        Microsoft.Win32.RegistryHive.LocalMachine, view))
                    using (Microsoft.Win32.RegistryKey k = hive.OpenSubKey(key))
                    {
                        object v = k == null ? null : k.GetValue("DisplayVersion");
                        if (v != null && v.ToString().Length > 0) return v.ToString();
                    }
                }
                catch { }
            }
            // The installer's entry can be missing while the driver itself is
            // installed and working (seen on the author's machine); the
            // kernel service is the real thing.
            try
            {
                using (Microsoft.Win32.RegistryKey k = Microsoft.Win32.Registry.LocalMachine.OpenSubKey(
                    @"SYSTEM\CurrentControlSet\Services\PawnIO"))
                {
                    if (k != null) return "installed";
                }
            }
            catch { }
            return "";
        }

        /// The current reading as JSON. Refreshes the fast hardware when the
        /// last reading is older than ~0.8 s (or always, with force); the
        /// disks come as last read by the storage worker. Before the fast
        /// hardware is open the answer has an empty hardware list and
        /// "starting": true -- the service is alive, just not ready.
        public string Snapshot(bool force)
        {
            lock (_gate)
            {
                if (!force && (DateTime.UtcNow - _cachedAt).TotalMilliseconds < 800 && _cached.Length > 0)
                    return _cached;

                StringBuilder sb = new StringBuilder(16 * 1024);
                sb.Append("{\"v\":1,\"service\":").Append(Json.Str(Program.Version));
                string pv = PawnIoVersion();
                sb.Append(",\"pawnio\":").Append(pv.Length > 0 ? "true" : "false");
                sb.Append(",\"pawnio_version\":").Append(Json.Str(pv));
                sb.Append(",\"starting\":").Append(_computer == null ? "true" : "false");
                sb.Append(",\"started\":").Append(Json.Str(_openedAt.ToString("o", CultureInfo.InvariantCulture)));
                sb.Append(",\"storage\":").Append(Json.Str(_storage.State));
                sb.Append(",\"hardware\":[");
                bool first = true;
                if (_computer != null)
                {
                    Interlocked.Exchange(ref BusySince, DateTime.UtcNow.Ticks);
                    try
                    {
                        foreach (IHardware h in _computer.Hardware)
                        {
                            Emit.Hardware(sb, h, null, ref first, true, ref _lastError);
                        }
                    }
                    finally
                    {
                        Interlocked.Exchange(ref BusySince, 0);
                    }
                    string disks = _storage.Fragment;
                    if (disks.Length > 0)
                    {
                        if (!first) sb.Append(',');
                        sb.Append(disks);
                    }
                }
                sb.Append("],\"error\":").Append(Json.Str(_lastError)).Append('}');
                string json = sb.ToString();
                if (_computer != null)
                {
                    _cached = json;
                    _cachedAt = DateTime.UtcNow;
                }
                return json;
            }
        }
    }

    /// Disks, on their own thread: LibreHardwareMonitor's storage group plus
    /// the USB drive temperatures LHM cannot see (UsbDriveTemps). Every few
    /// seconds it reads them and leaves the JSON for Monitor to pick up, so
    /// a slow or stuck drive only ever delays disk temperatures.
    ///
    /// A drive whose update takes longer than 10 s is left out for the next
    /// 10 minutes, so one bad enclosure does not hold up the other disks.
    internal sealed class StorageWorker
    {
        private const int EverySeconds = 3;
        private const int SlowSeconds = 10;
        private const int SkipMinutes = 10;

        private Computer _computer;
        private readonly UsbDriveTemps _usb = new UsbDriveTemps();
        private readonly Dictionary<string, DateTime> _skipUntil = new Dictionary<string, DateTime>();
        private string _drives = "";
        private volatile string _fragment = "";
        private volatile string _state = "opening";
        private Thread _thread;
        private ManualResetEvent _stop;

        /// What the storage worker is doing: "opening", "ok", or "busy: <what>".
        public string State { get { return _state; } }
        /// Comma-separated hardware objects for the disks, "" when none yet.
        public string Fragment { get { return _fragment; } }

        /// Ticks at which the current step began, 0 while idle, and what it is.
        public long BusySince;
        public volatile string BusyOn = "";

        public void Start(ManualResetEvent stop)
        {
            _stop = stop;
            _thread = new Thread(Run);
            _thread.IsBackground = true;
            _thread.Name = "storage";
            _thread.Start();
        }

        private void Busy(string what)
        {
            BusyOn = what;
            _state = "busy: " + what;
            Interlocked.Exchange(ref BusySince, DateTime.UtcNow.Ticks);
        }

        private void Idle()
        {
            Interlocked.Exchange(ref BusySince, 0);
            BusyOn = "";
        }

        private static string DriveSignature()
        {
            try { return string.Join(",", Environment.GetLogicalDrives()); }
            catch { return ""; }
        }

        private void Run()
        {
            while (!_stop.WaitOne(0))
            {
                try
                {
                    if (_computer == null)
                    {
                        _state = "opening";
                        Busy("opening disks");
                        Computer c = new Computer();
                        c.IsStorageEnabled = true;
                        Stopwatch sw = Stopwatch.StartNew();
                        c.Open();
                        _computer = c;
                        _drives = DriveSignature();
                        Idle();
                        Log.Write("disks opened in " + sw.ElapsedMilliseconds + " ms: " + Emit.Describe(c));
                    }
                    CheckDrives();
                    Read();
                    _state = "ok";
                }
                catch (Exception e)
                {
                    Idle();
                    _state = "error";
                    Log.Write("disk reading failed: " + e.Message);
                }
                if (_stop.WaitOne(EverySeconds * 1000)) break;
            }
        }

        /// A drive letter came or went: enumerate the disks again, so a USB
        /// drive plugged in later gets its temperature too.
        private void CheckDrives()
        {
            string now = DriveSignature();
            if (now == _drives) return;
            _drives = now;
            Busy("re-enumerating disks");
            Stopwatch sw = Stopwatch.StartNew();
            try
            {
                _computer.IsStorageEnabled = false;
                _computer.IsStorageEnabled = true;
                _usb.Invalidate();
                _skipUntil.Clear();
                Log.Write("drives changed (" + now + "), disks re-enumerated in " + sw.ElapsedMilliseconds + " ms: " +
                    Emit.Describe(_computer));
            }
            finally
            {
                Idle();
            }
        }

        private void Read()
        {
            StringBuilder sb = new StringBuilder(8 * 1024);
            bool first = true;
            string error = "";
            foreach (IHardware h in _computer.Hardware)
            {
                string key = h.Identifier.ToString();
                DateTime until;
                bool skipped = _skipUntil.TryGetValue(key, out until) && DateTime.UtcNow < until;
                Busy(h.Name);
                Stopwatch sw = Stopwatch.StartNew();
                try
                {
                    // A drive on the bench still shows (capacity, last values);
                    // it is just not asked again until the time is up.
                    Emit.Hardware(sb, h, null, ref first, !skipped, ref error);
                }
                finally
                {
                    Idle();
                }
                if (!skipped && sw.Elapsed.TotalSeconds > SlowSeconds)
                {
                    _skipUntil[key] = DateTime.UtcNow.AddMinutes(SkipMinutes);
                    Log.Write("disk \"" + h.Name + "\" took " + (int)sw.Elapsed.TotalSeconds +
                        " s to read; leaving it alone for " + SkipMinutes + " minutes");
                }
            }

            // Publish the disks LibreHardwareMonitor read right away (with the
            // USB temperatures from last time): asking a USB enclosure
            // directly can take a while and must not hold these up.
            string lhm = sb.ToString();
            _fragment = WithUsb(lhm, first, _usb.Last);
            if (!_announced)
            {
                _announced = true;
                Log.Write("disks: first reading, " + TempSummary());
            }

            Busy("USB drive temperatures");
            try
            {
                _fragment = WithUsb(lhm, first, _usb.Read(Covered()));
            }
            catch (Exception e)
            {
                Log.Write("usb drive temperatures failed: " + e.Message);
            }
            finally
            {
                Idle();
            }
        }

        private bool _announced;

        private static string WithUsb(string lhm, bool empty, List<KeyValuePair<string, float>> usb)
        {
            StringBuilder sb = new StringBuilder(lhm);
            bool first = empty;
            foreach (KeyValuePair<string, float> u in usb) EmitUsb(sb, u.Key, u.Value, ref first);
            return sb.ToString();
        }

        /// "WD_BLACK SN770 1TB 41 C, WDC WD10SMZW 35 C" for the log.
        private string TempSummary()
        {
            List<string> parts = new List<string>();
            foreach (IHardware h in _computer.Hardware)
            {
                string t = "no temperature";
                foreach (ISensor s in h.Sensors)
                {
                    if (s.SensorType == SensorType.Temperature && s.Value.HasValue)
                    {
                        t = s.Value.Value.ToString("0", CultureInfo.InvariantCulture) + " C";
                        break;
                    }
                }
                parts.Add(h.Name + " " + t);
            }
            return parts.Count == 0 ? "no disks" : string.Join(", ", parts.ToArray());
        }

        /// Physical drive numbers LibreHardwareMonitor already has a
        /// temperature for (its storage identifiers end in the drive number,
        /// "/hdd/2").
        private HashSet<int> Covered()
        {
            HashSet<int> set = new HashSet<int>();
            foreach (IHardware h in _computer.Hardware)
            {
                if (h.HardwareType != HardwareType.Storage) continue;
                bool hasTemp = false;
                foreach (ISensor s in h.Sensors)
                {
                    if (s.SensorType == SensorType.Temperature && s.Value.HasValue && s.Value.Value > 0) { hasTemp = true; break; }
                }
                if (!hasTemp) continue;
                string id = h.Identifier.ToString();
                int slash = id.LastIndexOf('/');
                int n;
                if (slash >= 0 && int.TryParse(id.Substring(slash + 1), NumberStyles.Integer, CultureInfo.InvariantCulture, out n))
                    set.Add(n);
            }
            return set;
        }

        /// A USB drive temperature from UsbDriveTemps, as a Storage entry
        /// named after its drive letter ("USB disk E:"; perf.js matches on
        /// that). The parent keeps it out of the app's hardware list, where
        /// the drive may already be counted.
        private static void EmitUsb(StringBuilder sb, string letter, float t, ref bool first)
        {
            if (!first) sb.Append(',');
            first = false;
            sb.Append("{\"name\":").Append(Json.Str("USB disk " + letter));
            sb.Append(",\"type\":\"Storage\",\"parent\":").Append(Json.Str("EarthDesk"));
            sb.Append(",\"sensors\":[{\"name\":\"Temperature\",\"type\":\"Temperature\",\"value\":");
            sb.Append(Json.Num(t)).Append("}]}");
        }
    }

    /// Temperatures of USB drives that LibreHardwareMonitor shows none for.
    ///
    /// Many USB enclosures (and the SAS-to-SATA boards on some second-hand
    /// server drives) pass only a few SMART attributes through, without
    /// 194/190 "temperature" -- so LibreHardwareMonitor, CrystalDiskInfo and
    /// Hard Disk Sentinel all show "?". The drive still answers when asked
    /// directly through the enclosure's SCSI/ATA translation (SAT, what
    /// smartctl -d sat does): the Device Statistics log, page 5, holds the
    /// current temperature; SMART attribute 194/190 is the second choice.
    ///
    /// Read-only commands only, at most once a minute per drive, and never on
    /// a drive that is spun down (CHECK POWER MODE first), so it does not keep
    /// a sleeping disk awake. Anything a bridge does not understand just
    /// means no temperature for that drive.
    internal sealed class UsbDriveTemps
    {
        private const int IntervalSeconds = 60;
        private DateTime _checkedAt = DateTime.MinValue;
        private List<KeyValuePair<string, float>> _last = new List<KeyValuePair<string, float>>();
        private readonly Dictionary<int, string> _state = new Dictionary<int, string>();
        /// Per physical drive: not before this time again. A drive whose
        /// enclosure passes none of the commands costs ~12 s of timeouts per
        /// attempt, so it is asked again only every half hour.
        private readonly Dictionary<int, DateTime> _next = new Dictionary<int, DateTime>();
        private readonly Dictionary<int, float> _temp = new Dictionary<int, float>();
        private const int GiveUpMinutes = 30;

        /// The last result, without asking any drive.
        public List<KeyValuePair<string, float>> Last { get { return _last; } }

        /// Drive letters changed: look again on the next reading.
        public void Invalidate()
        {
            _checkedAt = DateTime.MinValue;
            _next.Clear();
            _temp.Clear();
        }

        /// (drive letter like "E:", degrees C) for USB drives not in covered
        /// (physical drive numbers that already have a temperature).
        public List<KeyValuePair<string, float>> Read(HashSet<int> covered)
        {
            if ((DateTime.UtcNow - _checkedAt).TotalSeconds < IntervalSeconds) return _last;
            _checkedAt = DateTime.UtcNow;

            Dictionary<int, List<string>> letters = new Dictionary<int, List<string>>();
            DriveInfo[] drives;
            try { drives = DriveInfo.GetDrives(); }
            catch { drives = new DriveInfo[0]; }
            foreach (DriveInfo d in drives)
            {
                try
                {
                    // Never touch network or optical drives: those can block.
                    if (d.DriveType != DriveType.Fixed && d.DriveType != DriveType.Removable) continue;
                    string letter = d.Name.Substring(0, 2).ToUpperInvariant();
                    int n = Native.DiskNumber(letter);
                    if (n < 0) continue;
                    List<string> list;
                    if (!letters.TryGetValue(n, out list)) { list = new List<string>(); letters[n] = list; }
                    list.Add(letter);
                }
                catch { }
            }

            List<KeyValuePair<string, float>> result = new List<KeyValuePair<string, float>>();
            foreach (KeyValuePair<int, List<string>> e in letters)
            {
                string state;
                float t = float.NaN;
                DateTime next;
                if (_next.TryGetValue(e.Key, out next) && DateTime.UtcNow < next)
                {
                    // Not due yet: keep what it said last time.
                    float kept;
                    if (_temp.TryGetValue(e.Key, out kept))
                        foreach (string letter in e.Value) result.Add(new KeyValuePair<string, float>(letter, kept));
                    continue;
                }
                try
                {
                    if (covered.Contains(e.Key)) state = "covered by LibreHardwareMonitor";
                    else if (!Native.IsUsb(e.Key)) state = "not USB";
                    else state = Native.Temperature(e.Key, out t);
                }
                catch (Exception ex)
                {
                    state = "error: " + ex.Message;
                }
                bool unanswered = state.StartsWith("no temperature") || state.StartsWith("cannot open") || state.StartsWith("error");
                _next[e.Key] = unanswered ? DateTime.UtcNow.AddMinutes(GiveUpMinutes) : DateTime.MinValue;
                if (float.IsNaN(t)) _temp.Remove(e.Key);
                else _temp[e.Key] = t;
                string previous;
                if (!_state.TryGetValue(e.Key, out previous) || previous != state)
                {
                    _state[e.Key] = state;
                    Log.Write("usb drive temperature, disk " + e.Key + " (" +
                        string.Join(",", e.Value.ToArray()) + "): " + state +
                        (float.IsNaN(t) ? "" : ", " + t.ToString(CultureInfo.InvariantCulture) + " C"));
                }
                if (float.IsNaN(t)) continue;
                foreach (string letter in e.Value) result.Add(new KeyValuePair<string, float>(letter, t));
            }
            _last = result;
            return result;
        }
    }

    /// Win32 plumbing for UsbDriveTemps.
    internal static class Native
    {
        private const uint GENERIC_READ = 0x80000000, GENERIC_WRITE = 0x40000000;
        private const uint FILE_SHARE_READ = 1, FILE_SHARE_WRITE = 2, OPEN_EXISTING = 3;
        private const uint IOCTL_STORAGE_GET_DEVICE_NUMBER = 0x002D1080;
        private const uint IOCTL_STORAGE_QUERY_PROPERTY = 0x002D1400;
        private const uint IOCTL_SCSI_PASS_THROUGH = 0x0004D004;
        private const int BusTypeUsb = 7;

        [System.Runtime.InteropServices.DllImport("kernel32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
        private static extern Microsoft.Win32.SafeHandles.SafeFileHandle CreateFile(string name, uint access,
            uint share, IntPtr security, uint disposition, uint flags, IntPtr template);

        [System.Runtime.InteropServices.DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool DeviceIoControl(Microsoft.Win32.SafeHandles.SafeFileHandle device, uint code,
            byte[] inBuffer, int inSize, byte[] outBuffer, int outSize, out int returned, IntPtr overlapped);

        private static Microsoft.Win32.SafeHandles.SafeFileHandle Open(string path, uint access)
        {
            return CreateFile(path, access, FILE_SHARE_READ | FILE_SHARE_WRITE, IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero);
        }

        /// Physical drive number behind a volume like "E:", or -1.
        public static int DiskNumber(string letter)
        {
            using (Microsoft.Win32.SafeHandles.SafeFileHandle h = Open(@"\\.\" + letter, 0))
            {
                if (h.IsInvalid) return -1;
                byte[] o = new byte[12];
                int got;
                if (!DeviceIoControl(h, IOCTL_STORAGE_GET_DEVICE_NUMBER, null, 0, o, o.Length, out got, IntPtr.Zero) || got < 8)
                    return -1;
                // STORAGE_DEVICE_NUMBER { DeviceType, DeviceNumber, PartitionNumber }
                return BitConverter.ToInt32(o, 4);
            }
        }

        public static bool IsUsb(int disk)
        {
            using (Microsoft.Win32.SafeHandles.SafeFileHandle h = Open(@"\\.\PhysicalDrive" + disk, 0))
            {
                if (h.IsInvalid) return false;
                byte[] q = new byte[12]; // StorageDeviceProperty, PropertyStandardQuery
                byte[] o = new byte[1024];
                int got;
                if (!DeviceIoControl(h, IOCTL_STORAGE_QUERY_PROPERTY, q, q.Length, o, o.Length, out got, IntPtr.Zero) || got < 32)
                    return false;
                // STORAGE_DEVICE_DESCRIPTOR.BusType
                return BitConverter.ToInt32(o, 28) == BusTypeUsb;
            }
        }

        /// How the temperature was found ("device statistics" / "SMART 194"
        /// ...) or why not; t is NaN unless one was found.
        public static string Temperature(int disk, out float t)
        {
            t = float.NaN;
            using (Microsoft.Win32.SafeHandles.SafeFileHandle h = Open(@"\\.\PhysicalDrive" + disk, GENERIC_READ | GENERIC_WRITE))
            {
                if (h.IsInvalid) return "cannot open, error " + System.Runtime.InteropServices.Marshal.GetLastWin32Error();

                // CHECK POWER MODE (E5h), non-data, results wanted (ck_cond).
                // Count 00h = standby: leave the drive asleep.
                byte[] cdb = Cdb(3, false, false, 0, 0, 0, 0, 0xE5);
                cdb[2] = 0x20;
                byte[] sense, data;
                if (Send(h, cdb, 0, out sense, out data))
                {
                    int count = ResultCount(sense);
                    if (count == 0) return "asleep";
                }

                // READ LOG EXT (2Fh): Device Statistics log (04h), page 05h.
                if (Send(h, Cdb(4, true, true, 0, 1, 0x04, 0x05, 0x2F), 512, out sense, out data) &&
                    data[2] == 0x05 && (data[15] & 0xC0) == 0xC0)
                {
                    int v = (sbyte)data[8];
                    if (v > 0 && v < 100) { t = v; return "device statistics"; }
                }

                // SMART READ DATA (B0h / D0h): attribute 194, else 190.
                if (Send(h, Cdb(4, false, true, 0xD0, 1, 0, 0x4F, 0xB0, 0xC2), 512, out sense, out data))
                {
                    int best = -1, bestId = 0;
                    for (int i = 0; i < 30; i++)
                    {
                        int at = 2 + i * 12;
                        int id = data[at];
                        int v = data[at + 5];
                        if ((id == 194 || (id == 190 && bestId != 194)) && v > 0 && v < 100) { best = v; bestId = id; }
                    }
                    if (best > 0) { t = best; return "SMART " + bestId; }
                    return ScsiTemperature(h, out t) ? "SCSI log page 0Dh" : "no temperature (SMART has no 194/190)";
                }
                string ataWhy = _why;
                // Not an ATA drive behind a SAT bridge (a SAS drive, or a bridge
                // that only speaks SCSI): ask the SCSI way.
                if (ScsiTemperature(h, out t)) return "SCSI log page 0Dh";
                // Say what actually came back: error 1117 (ERROR_IO_DEVICE) or
                // a timeout means the drive/bridge did not answer at all, which
                // is not the same as "this enclosure cannot do it".
                return "no temperature (ATA: " + ataWhy + "; SCSI: " + _why + ")";
            }
        }

        /// ATA PASS-THROUGH (16), SAT. protocol 3 = non-data, 4 = PIO data-in.
        private static byte[] Cdb(int protocol, bool ext, bool dataIn, int features, int count,
            int lbaLow, int lbaMid, int command, int lbaHigh = 0)
        {
            byte[] c = new byte[16];
            c[0] = 0x85;
            c[1] = (byte)((protocol << 1) | (ext ? 1 : 0));
            // t_dir = from device, byte_block = 1, t_length = the count field
            c[2] = (byte)(dataIn ? 0x0E : 0x00);
            c[4] = (byte)features;
            c[6] = (byte)count;
            c[8] = (byte)lbaLow;
            c[10] = (byte)lbaMid;
            c[12] = (byte)lbaHigh;
            c[14] = (byte)command;
            return c;
        }

        /// Sends one CDB through IOCTL_SCSI_PASS_THROUGH (buffered: the
        /// SCSI_PASS_THROUGH header, sense and data share one buffer).
        /// True when the command completed; for ck_cond commands a CHECK
        /// CONDITION status carrying the ATA results also counts.
        private static bool Send(Microsoft.Win32.SafeHandles.SafeFileHandle h, byte[] cdb, int dataLength,
            out byte[] sense, out byte[] data)
        {
            bool x64 = IntPtr.Size == 8;
            int header = x64 ? 56 : 44;
            int senseOff = header, senseLen = 32;
            int dataOff = header + senseLen;
            byte[] buf = new byte[dataOff + Math.Max(dataLength, 0)];
            BitConverter.GetBytes((ushort)header).CopyTo(buf, 0);          // Length
            buf[6] = (byte)cdb.Length;                                     // CdbLength
            buf[7] = (byte)senseLen;                                       // SenseInfoLength
            buf[8] = (byte)(dataLength > 0 ? 1 : 2);                       // DataIn: IN / UNSPECIFIED
            BitConverter.GetBytes(dataLength).CopyTo(buf, 12);             // DataTransferLength
            BitConverter.GetBytes(5).CopyTo(buf, 16);                      // TimeOutValue, s
            if (x64)
            {
                BitConverter.GetBytes((long)dataOff).CopyTo(buf, 24);      // DataBufferOffset
                BitConverter.GetBytes(senseOff).CopyTo(buf, 32);           // SenseInfoOffset
                Array.Copy(cdb, 0, buf, 36, Math.Min(16, cdb.Length));
            }
            else
            {
                BitConverter.GetBytes(dataOff).CopyTo(buf, 20);
                BitConverter.GetBytes(senseOff).CopyTo(buf, 24);
                Array.Copy(cdb, 0, buf, 28, Math.Min(16, cdb.Length));
            }

            sense = new byte[senseLen];
            data = new byte[Math.Max(dataLength, 0)];
            int got;
            Stopwatch sw = Stopwatch.StartNew();
            if (!DeviceIoControl(h, IOCTL_SCSI_PASS_THROUGH, buf, buf.Length, buf, buf.Length, out got, IntPtr.Zero))
            {
                _why = "cmd " + cdb[cdb[0] == 0x85 ? 14 : 0].ToString("X2") + " ioctl error " +
                    System.Runtime.InteropServices.Marshal.GetLastWin32Error() + " after " + sw.ElapsedMilliseconds + " ms";
                return false;
            }
            Array.Copy(buf, senseOff, sense, 0, senseLen);
            if (dataLength > 0) Array.Copy(buf, dataOff, data, 0, dataLength);
            byte status = buf[2];
            if (status == 0)
            {
                bool ok = dataLength == 0 || BitConverter.ToInt32(buf, 12) > 0;
                if (!ok) _why = "cmd " + cdb[cdb[0] == 0x85 ? 14 : 0].ToString("X2") + " returned no data";
                return ok;
            }
            _why = "cmd " + cdb[cdb[0] == 0x85 ? 14 : 0].ToString("X2") + " scsi status " + status +
                " sense " + BitConverter.ToString(sense, 0, 14);
            return dataLength == 0 && status == 2 && (cdb[2] & 0x20) != 0;
        }

        /// The ATA Count register from ck_cond sense data, or -1.
        /// LOG SENSE, Temperature page (0Dh), current values: what
        /// "smartctl -d scsi" reports as "Current Drive Temperature". SAS
        /// drives answer it natively; many SAT bridges translate it from
        /// the ATA drive's SMART data too.
        private static bool ScsiTemperature(Microsoft.Win32.SafeHandles.SafeFileHandle h, out float t)
        {
            t = float.NaN;
            byte[] cdb = new byte[10];
            cdb[0] = 0x4D;               // LOG SENSE
            cdb[2] = 0x40 | 0x0D;        // PC = current cumulative, page 0Dh
            cdb[8] = 64;                 // allocation length
            byte[] sense, data;
            if (!Send(h, cdb, 64, out sense, out data) || (data[0] & 0x3F) != 0x0D) return false;
            int end = Math.Min(data.Length, 4 + ((data[2] << 8) | data[3]));
            for (int i = 4; i + 4 <= end; i += 4 + data[i + 3])
            {
                int code = (data[i] << 8) | data[i + 1];
                int len = data[i + 3];
                // Parameter 0000h: byte 1 of the value is degrees C, FFh = unknown.
                if (code == 0 && len >= 2 && i + 5 < data.Length)
                {
                    int v = data[i + 5];
                    if (v > 0 && v < 100) { t = v; return true; }
                    return false;
                }
            }
            return false;
        }

        /// Why the last Send failed, for the log.
        [ThreadStatic] private static string _why;

        private static int ResultCount(byte[] sense)
        {
            int code = sense[0] & 0x7F;
            // Only sense that says "ATA PASS-THROUGH information available"
            // (RECOVERED ERROR or NO SENSE, 00h/1Dh) carries the registers. Anything else
            // -- a bridge rejecting ck_cond with ILLEGAL REQUEST, say -- used to
            // be read as count 0 = "asleep", and the drive was never asked for
            // its temperature again.
            if (code == 0x72 || code == 0x73)
            {
                if ((sense[1] & 0x0F) > 0x01 || sense[2] != 0x00 || sense[3] != 0x1D) return -1;
            }
            else if (code == 0x70 || code == 0x71)
            {
                if ((sense[2] & 0x0F) > 0x01 || sense[12] != 0x00 || sense[13] != 0x1D) return -1;
            }
            if (code == 0x72 || code == 0x73)
            {
                int end = Math.Min(sense.Length, 8 + sense[7]);
                for (int i = 8; i + 1 < end; i += 2 + sense[i + 1])
                {
                    if (sense[i] == 0x09 && i + 5 < end) return sense[i + 5];
                }
                return -1;
            }
            if (code == 0x70 || code == 0x71) return sense[6];
            return -1;
        }
    }

    internal static class Json
    {
        public static string Str(string s)
        {
            if (s == null) return "null";
            StringBuilder sb = new StringBuilder(s.Length + 2);
            sb.Append('"');
            foreach (char c in s)
            {
                switch (c)
                {
                    case '"': sb.Append("\\\""); break;
                    case '\\': sb.Append("\\\\"); break;
                    case '\n': sb.Append("\\n"); break;
                    case '\r': sb.Append("\\r"); break;
                    case '\t': sb.Append("\\t"); break;
                    default:
                        if (c < 0x20) sb.Append("\\u").Append(((int)c).ToString("x4"));
                        else sb.Append(c);
                        break;
                }
            }
            sb.Append('"');
            return sb.ToString();
        }

        public static string Num(float v)
        {
            return Math.Round((double)v, 3).ToString("R", CultureInfo.InvariantCulture);
        }

        public static string Error(string message)
        {
            return "{\"v\":1,\"service\":" + Str(Program.Version) + ",\"hardware\":[],\"error\":" + Str(message) + "}";
        }
    }

    /// Opens the hardware on a worker thread, serves the pipe, and keeps an
    /// eye on both.
    internal sealed class Server
    {
        private readonly Monitor _monitor = new Monitor();
        private readonly ManualResetEvent _stop = new ManualResetEvent(false);
        private Thread _worker;
        private Thread _watchdog;
        private bool _diskStuckLogged;

        public void Start()
        {
            Log.Write("start, version " + Program.Version + ", pid " + Process.GetCurrentProcess().Id);
            _worker = new Thread(Run);
            _worker.IsBackground = true;
            _worker.Name = "pipe";
            _worker.Start();
            _watchdog = new Thread(Watch);
            _watchdog.IsBackground = true;
            _watchdog.Name = "watchdog";
            _watchdog.Start();
        }

        public void Stop()
        {
            Log.Write("stop");
            _stop.Set();
            if (_worker != null) _worker.Join(5000);
            _monitor.Close();
        }

        private static PipeSecurity Security()
        {
            PipeSecurity ps = new PipeSecurity();
            ps.AddAccessRule(new PipeAccessRule(new SecurityIdentifier(WellKnownSidType.LocalSystemSid, null),
                PipeAccessRights.FullControl, AccessControlType.Allow));
            ps.AddAccessRule(new PipeAccessRule(new SecurityIdentifier(WellKnownSidType.BuiltinAdministratorsSid, null),
                PipeAccessRights.FullControl, AccessControlType.Allow));
            // Anyone signed in on this computer may read the numbers ...
            ps.AddAccessRule(new PipeAccessRule(new SecurityIdentifier(WellKnownSidType.AuthenticatedUserSid, null),
                PipeAccessRights.Read | PipeAccessRights.Synchronize, AccessControlType.Allow));
            // ... but nobody coming in over the network.
            ps.AddAccessRule(new PipeAccessRule(new SecurityIdentifier(WellKnownSidType.NetworkSid, null),
                PipeAccessRights.FullControl, AccessControlType.Deny));
            return ps;
        }

        /// Opens the fast hardware (retrying: a driver that is still starting
        /// at boot usually answers a little later), then starts the disks.
        /// Runs beside the pipe, which answers "starting" until this is done.
        private void OpenAll()
        {
            for (int attempt = 0; ; attempt++)
            {
                try
                {
                    _monitor.Open();
                    break;
                }
                catch (Exception e)
                {
                    Log.Write("open failed: " + e);
                    if (_stop.WaitOne(Math.Min(60, 5 << Math.Min(attempt, 4)) * 1000)) return;
                }
            }
            _monitor.Storage.Start(_stop);

            // Started together with Windows: graphics drivers can come up
            // after us, and LibreHardwareMonitor only looks for GPUs when it
            // opens. Look once more a little later if none was found.
            bool bootStart = Environment.TickCount > 0 && Environment.TickCount < 5 * 60 * 1000;
            if (bootStart && !_monitor.HasGpu() && !_stop.WaitOne(90000))
            {
                try { _monitor.Reopen("no GPU found at boot"); }
                catch (Exception e) { Log.Write("reopen failed: " + e.Message); }
            }
        }

        private void Run()
        {
            Thread opener = new Thread(OpenAll);
            opener.IsBackground = true;
            opener.Name = "open";
            opener.Start();

            PipeSecurity security = Security();
            while (!_stop.WaitOne(0))
            {
                NamedPipeServerStream pipe = null;
                try
                {
                    pipe = new NamedPipeServerStream(Program.PipeName, PipeDirection.Out, 1,
                        PipeTransmissionMode.Byte, PipeOptions.Asynchronous, 0, 256 * 1024, security);
                    IAsyncResult ar = pipe.BeginWaitForConnection(null, null);
                    int which = WaitHandle.WaitAny(new WaitHandle[] { ar.AsyncWaitHandle, _stop }, 30000);
                    if (which == 1) break;
                    if (which == WaitHandle.WaitTimeout) continue;
                    pipe.EndWaitForConnection(ar);

                    string json;
                    try
                    {
                        json = _monitor.Snapshot(false);
                    }
                    catch (Exception e)
                    {
                        Log.Write("snapshot failed: " + e);
                        json = Json.Error(e.Message);
                    }
                    byte[] bytes = Encoding.UTF8.GetBytes(json);
                    pipe.Write(bytes, 0, bytes.Length);
                    pipe.Flush();
                    pipe.WaitForPipeDrain();
                }
                catch (IOException)
                {
                    // The client went away before reading everything. Fine.
                }
                catch (Exception e)
                {
                    Log.Write("pipe error: " + e.Message);
                    if (_stop.WaitOne(1000)) break;
                }
                finally
                {
                    if (pipe != null)
                    {
                        try { pipe.Dispose(); }
                        catch { }
                    }
                }
            }
        }

        /// Exit (and let the service manager restart us) when a hardware
        /// update has hung, or the process has grown far past its normal size.
        /// Exiting without telling the SCM counts as a crash, which is exactly
        /// what triggers the recovery action.
        private void Watch()
        {
            while (!_stop.WaitOne(10000))
            {
                long since = Interlocked.Read(ref _monitor.BusySince);
                if (since != 0 && (DateTime.UtcNow.Ticks - since) > TimeSpan.FromSeconds(60).Ticks)
                {
                    Log.Write("hardware update hung for 60 s; exiting so the service restarts");
                    Die();
                }
                // A stuck disk only delays disk readings (see StorageWorker),
                // and restarting would just get stuck on it again; say so once.
                long disk = Interlocked.Read(ref _monitor.Storage.BusySince);
                if (disk != 0 && (DateTime.UtcNow.Ticks - disk) > TimeSpan.FromSeconds(60).Ticks)
                {
                    if (!_diskStuckLogged)
                    {
                        _diskStuckLogged = true;
                        Log.Write("disks: \"" + _monitor.Storage.BusyOn + "\" has not returned for 60 s; CPU, GPU and fans carry on without it");
                    }
                }
                else
                {
                    _diskStuckLogged = false;
                }
                long bytes = 0;
                try
                {
                    Process me = Process.GetCurrentProcess();
                    me.Refresh();
                    bytes = me.PrivateMemorySize64;
                }
                catch { }
                if (bytes > 400L * 1024 * 1024)
                {
                    Log.Write("private memory " + (bytes >> 20) + " MB; exiting so the service restarts");
                    Die();
                }
            }
        }

        /// Terminate at once. A thread stuck inside a driver call can keep a
        /// polite Environment.Exit waiting forever; TerminateProcess cannot.
        private static void Die()
        {
            try { Process.GetCurrentProcess().Kill(); }
            catch { Environment.Exit(3); }
        }
    }
}
